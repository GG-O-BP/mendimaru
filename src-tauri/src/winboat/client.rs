use super::container::http_client;
use crate::config::runtime_host_port_async;
use crate::models::{AppConfig, StudioVersion, WinApp};
use futures_util::StreamExt;
use regex::Regex;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const GUEST_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const GUEST_HEALTH_TIMEOUT_SECONDS: u64 = 2;
const PROJECTED_APPS_TIMEOUT_SECONDS: u64 = 12;
const MAX_APPS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROJECTED_APPS_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_HEALTH_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_GUEST_TOKEN_BYTES: u64 = 4 * 1024;
const MAX_LEGACY_APPS: usize = 4_096;
const MAX_PROJECTED_APPS: usize = 128;
const MAX_CAPABILITIES: usize = 64;
const MAX_CAPABILITY_BYTES: usize = 128;
const MAX_APP_NAME_BYTES: usize = 4 * 1024;
const MAX_APP_PATH_BYTES: usize = 32 * 1024;
const MAX_APP_SOURCE_BYTES: usize = 4 * 1024;
const APPS_QUERY_CAPABILITY: &str = "apps-query-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuestHealth {
    status: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    authentication: Option<String>,
}

impl GuestHealth {
    fn validate(&self) -> Result<(), String> {
        if self.status != "ok" {
            return Err("health status is not ok".to_string());
        }
        if self.capabilities.len() > MAX_CAPABILITIES
            || self.capabilities.iter().any(|capability| {
                capability.is_empty()
                    || capability.len() > MAX_CAPABILITY_BYTES
                    || capability.chars().any(char::is_control)
            })
        {
            return Err("health capabilities exceed the safe limits".to_string());
        }
        if self
            .authentication
            .as_deref()
            .is_some_and(|authentication| {
                authentication.is_empty()
                    || authentication.len() > MAX_CAPABILITY_BYTES
                    || authentication.chars().any(char::is_control)
            })
        {
            return Err("health authentication metadata is invalid".to_string());
        }
        Ok(())
    }

    fn supports_projected_apps(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == APPS_QUERY_CAPABILITY)
    }
}

#[derive(Debug, Clone, Copy)]
enum AppsRequestMode {
    Projected,
    Legacy,
}

impl AppsRequestMode {
    const fn response_limit(self) -> usize {
        match self {
            Self::Projected => MAX_PROJECTED_APPS_RESPONSE_BYTES,
            Self::Legacy => MAX_APPS_RESPONSE_BYTES,
        }
    }

    const fn app_limit(self) -> usize {
        match self {
            Self::Projected => MAX_PROJECTED_APPS,
            Self::Legacy => MAX_LEGACY_APPS,
        }
    }

    const fn request_timeout(self) -> Duration {
        match self {
            Self::Projected => Duration::from_secs(PROJECTED_APPS_TIMEOUT_SECONDS),
            Self::Legacy => Duration::from_secs(GUEST_REQUEST_TIMEOUT_SECONDS),
        }
    }

    const fn trace_label(self) -> &'static str {
        match self {
            Self::Projected => "projected",
            Self::Legacy => "legacy",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ProjectedWinApp {
    name: String,
    path: String,
    source: String,
}

impl From<ProjectedWinApp> for WinApp {
    fn from(app: ProjectedWinApp) -> Self {
        Self {
            name: app.name,
            path: app.path,
            source: app.source,
        }
    }
}

pub async fn installed_versions(config: &AppConfig) -> Result<Vec<StudioVersion>, String> {
    super::version_cache::refresh(config, || fetch_installed_versions(config)).await
}

async fn fetch_installed_versions(config: &AppConfig) -> Result<Vec<StudioVersion>, String> {
    let started_at = Instant::now();
    let api_url = runtime_host_port_async(config, 7148, "tcp")
        .await
        .map_err(|error| crate::tr!("error-windows-apps-fetch", error = error))?
        .map(|port| format!("http://127.0.0.1:{port}"))
        .unwrap_or_else(|| config.api_url.clone());
    fetch_installed_versions_at(config, &api_url, started_at).await
}

async fn fetch_installed_versions_at(
    config: &AppConfig,
    api_url: &str,
    started_at: Instant,
) -> Result<Vec<StudioVersion>, String> {
    let authorization = load_guest_authorization(config)?;
    let client = http_client(Duration::from_secs(GUEST_REQUEST_TIMEOUT_SECONDS))?;
    let health = fetch_guest_health(&client, api_url).await?;
    let health_elapsed = started_at.elapsed();
    if health.authentication.as_deref() == Some("bearer") && authorization.is_none() {
        return Err(crate::tr!("error-guest-auth-required"));
    }
    if health
        .authentication
        .as_deref()
        .is_some_and(|authentication| authentication != "bearer")
    {
        return Err(crate::tr!(
            "error-windows-apps-response",
            error = "unsupported Guest authentication mode"
        ));
    }
    let mode = if health.supports_projected_apps() {
        AppsRequestMode::Projected
    } else {
        AppsRequestMode::Legacy
    };
    let request_started_at = Instant::now();
    let mut request = client.get(format!("{api_url}/apps"));
    let path_prefix = format!(
        "{}\\",
        normalize_windows_path(&config.mendix_install_root).trim_end_matches('\\')
    );
    let limit = MAX_PROJECTED_APPS.to_string();
    if matches!(mode, AppsRequestMode::Projected) {
        request = request.query(&[
            ("includeIcons", "false"),
            ("pathPrefix", path_prefix.as_str()),
            ("pathSuffix", r"\modeler\studiopro.exe"),
            ("fields", "Name,Path,Source"),
            ("limit", limit.as_str()),
        ]);
    }
    request = request.timeout(mode.request_timeout());
    let response = with_authorization(request, authorization.as_ref())
        .send()
        .await
        .map_err(|error| crate::tr!("error-windows-apps-fetch", error = error))?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(crate::tr!("error-guest-auth-required"));
    }
    let response = response
        .error_for_status()
        .map_err(|error| crate::tr!("error-windows-apps-response", error = error))?;
    let payload = read_bounded_response(response, mode.response_limit()).await?;
    let request_elapsed = request_started_at.elapsed();
    let parse_started_at = Instant::now();
    let apps = parse_apps_response(&payload, mode)?;
    let app_count = apps.len();
    let versions = parse_studio_versions(apps, &config.mendix_install_root);
    trace_guest_apps(
        mode,
        health_elapsed,
        request_elapsed,
        parse_started_at.elapsed(),
        payload.len(),
        app_count,
        versions.len(),
    );
    Ok(versions)
}

async fn fetch_guest_health(
    client: &reqwest::Client,
    api_url: &str,
) -> Result<GuestHealth, String> {
    let response = client
        .get(format!("{api_url}/health"))
        .timeout(Duration::from_secs(GUEST_HEALTH_TIMEOUT_SECONDS))
        .send()
        .await
        .map_err(|_| crate::tr!("error-guest-offline"))?;
    if !response.status().is_success() {
        return Err(crate::tr!("error-guest-offline"));
    }
    let payload = read_bounded_response(response, MAX_HEALTH_RESPONSE_BYTES).await?;
    let health = serde_json::from_slice::<GuestHealth>(&payload)
        .map_err(|error| crate::tr!("error-windows-apps-parse", error = error))?;
    health
        .validate()
        .map_err(|error| crate::tr!("error-windows-apps-parse", error = error))?;
    Ok(health)
}

async fn read_bounded_response(response: Response, max_bytes: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(crate::tr!(
            "error-windows-apps-parse",
            error = "response exceeds the safe size limit"
        ));
    }
    let mut payload = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| crate::tr!("error-windows-apps-fetch", error = error))?;
        if payload.len().saturating_add(chunk.len()) > max_bytes {
            return Err(crate::tr!(
                "error-windows-apps-parse",
                error = "response exceeds the safe size limit"
            ));
        }
        payload.extend_from_slice(&chunk);
    }
    Ok(payload)
}

fn parse_apps_response(payload: &[u8], mode: AppsRequestMode) -> Result<Vec<WinApp>, String> {
    let apps = match mode {
        AppsRequestMode::Projected => serde_json::from_slice::<Vec<ProjectedWinApp>>(payload)
            .map(|apps| apps.into_iter().map(WinApp::from).collect()),
        AppsRequestMode::Legacy => serde_json::from_slice::<Vec<WinApp>>(payload),
    }
    .map_err(|error| crate::tr!("error-windows-apps-parse", error = error))?;
    validate_apps(&apps, mode.app_limit())
        .map_err(|error| crate::tr!("error-windows-apps-parse", error = error))?;
    Ok(apps)
}

fn validate_apps(apps: &[WinApp], max_apps: usize) -> Result<(), String> {
    if apps.len() > max_apps {
        return Err("response contains too many apps".to_string());
    }
    for app in apps {
        if app.name.is_empty()
            || app.path.is_empty()
            || app.name.len() > MAX_APP_NAME_BYTES
            || app.path.len() > MAX_APP_PATH_BYTES
            || app.source.len() > MAX_APP_SOURCE_BYTES
            || [&app.name, &app.path, &app.source]
                .into_iter()
                .any(|field| field.chars().any(char::is_control))
        {
            return Err("response contains an invalid app field".to_string());
        }
    }
    Ok(())
}

fn with_authorization(
    request: RequestBuilder,
    authorization: Option<&HeaderValue>,
) -> RequestBuilder {
    match authorization {
        Some(authorization) => request.header(AUTHORIZATION, authorization.clone()),
        None => request,
    }
}

fn load_guest_authorization(config: &AppConfig) -> Result<Option<HeaderValue>, String> {
    let compose_file = Path::new(&config.compose_file);
    let Some(parent) = compose_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(None);
    };
    let token_path = parent.join("guest_token");
    let file = match open_guest_token(&token_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(crate::tr!("error-guest-auth-token")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| crate::tr!("error-guest-auth-token"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_GUEST_TOKEN_BYTES {
        return Err(crate::tr!("error-guest-auth-token"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and only reads the current process identity.
        let effective_user = unsafe { libc::geteuid() };
        if metadata.uid() != effective_user || metadata.mode() & 0o022 != 0 {
            return Err(crate::tr!("error-guest-auth-token"));
        }
    }
    let mut content = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    file.take(MAX_GUEST_TOKEN_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|_| crate::tr!("error-guest-auth-token"))?;
    if content.len() as u64 > MAX_GUEST_TOKEN_BYTES {
        return Err(crate::tr!("error-guest-auth-token"));
    }
    let token = std::str::from_utf8(&content)
        .map(str::trim)
        .map_err(|_| crate::tr!("error-guest-auth-token"))?;
    if token.len() < 16
        || token.len() > 512
        || !token.chars().all(|character| character.is_ascii_graphic())
    {
        return Err(crate::tr!("error-guest-auth-token"));
    }
    let mut bearer = Zeroizing::new(String::with_capacity("Bearer ".len() + token.len()));
    bearer.push_str("Bearer ");
    bearer.push_str(token);
    let mut header = HeaderValue::from_bytes(bearer.as_bytes())
        .map_err(|_| crate::tr!("error-guest-auth-token"))?;
    header.set_sensitive(true);
    Ok(Some(header))
}

fn open_guest_token(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

pub(super) async fn installed_versions_cached(
    config: &AppConfig,
) -> Result<Vec<StudioVersion>, String> {
    if let Some(versions) = super::version_cache::get(config) {
        return Ok(versions);
    }
    super::version_cache::refresh(config, || fetch_installed_versions(config)).await
}

fn trace_guest_apps(
    mode: AppsRequestMode,
    health: Duration,
    request: Duration,
    parse: Duration,
    payload_bytes: usize,
    app_count: usize,
    studio_count: usize,
) {
    if !crate::studio_trace::enabled() {
        return;
    }
    eprintln!(
        "[studio-overview] guest-apps mode={} health_ms={} request_ms={} parse_ms={} payload_bytes={payload_bytes} app_count={app_count} studio_count={studio_count}",
        mode.trace_label(),
        health.as_millis(),
        request.as_millis(),
        parse.as_millis(),
    );
}

pub(super) fn parse_studio_versions(apps: Vec<WinApp>, install_root: &str) -> Vec<StudioVersion> {
    let root = normalize_windows_path(install_root)
        .trim_end_matches('\\')
        .to_string();
    let prefix = format!("{}\\", root.to_lowercase());
    let version_pattern = Regex::new(r"^(\d+\.\d+\.\d+)(?:\.\d+)?$").expect("version regex");
    let mut versions = BTreeMap::<String, StudioVersion>::new();

    for app in apps {
        let normalized_path = normalize_windows_path(&app.path);
        let lower_path = normalized_path.to_lowercase();
        if !lower_path.starts_with(&prefix) || !lower_path.ends_with(r"\modeler\studiopro.exe") {
            continue;
        }
        let relative = &normalized_path[prefix.len()..];
        let Some(folder) = relative.split('\\').next() else {
            continue;
        };
        let Some(captures) = version_pattern.captures(folder) else {
            continue;
        };
        let version = captures
            .get(1)
            .expect("version capture")
            .as_str()
            .to_string();
        versions.entry(version.clone()).or_insert(StudioVersion {
            version: version.clone(),
            display_name: if app.name.is_empty() {
                format!("Studio Pro {version}")
            } else {
                app.name
            },
            executable_path: app.path,
            install_root: format!("{}\\{}", install_root.trim_end_matches('\\'), folder),
            source: if app.source.is_empty() {
                "WinBoat Guest Server".to_string()
            } else {
                app.source
            },
            removable: true,
        });
    }

    let mut result: Vec<_> = versions.into_values().collect();
    result.sort_by_key(|item| std::cmp::Reverse(version_parts(&item.version)));
    result
}

fn version_parts(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect()
}

fn normalize_windows_path(path: &str) -> String {
    path.replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContainerRuntime;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    struct FixtureResponse {
        status: u16,
        body: &'static str,
        content_length: Option<usize>,
    }

    impl FixtureResponse {
        fn json(body: &'static str) -> Self {
            Self {
                status: 200,
                body,
                content_length: None,
            }
        }

        fn status(status: u16) -> Self {
            Self {
                status,
                body: "",
                content_length: None,
            }
        }
    }

    fn fixture_server(
        responses: Vec<FixtureResponse>,
    ) -> (String, Arc<Mutex<Vec<String>>>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let worker = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("fixture read timeout");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1_024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).expect("read fixture request");
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() <= 64 * 1_024, "fixture request is bounded");
                }
                captured
                    .lock()
                    .expect("capture fixture request")
                    .push(String::from_utf8(request).expect("request is utf-8"));
                let reason = match response.status {
                    200 => "OK",
                    401 => "Unauthorized",
                    403 => "Forbidden",
                    _ => "Error",
                };
                let content_length = response.content_length.unwrap_or(response.body.len());
                let headers = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status, reason, content_length
                );
                stream
                    .write_all(headers.as_bytes())
                    .and_then(|_| stream.write_all(response.body.as_bytes()))
                    .expect("write fixture response");
            }
        });
        (format!("http://{address}"), requests, worker)
    }

    fn config(compose_file: &Path) -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: compose_file.to_string_lossy().into_owned(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat-fixture".into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47_300,
            shared_directory: "/tmp/workspace".into(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn initialize_i18n() {
        crate::i18n::initialize("en-US").expect("English localization initializes");
    }

    #[tokio::test]
    async fn uses_authenticated_projected_query_when_capability_is_advertised() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        std::fs::write(
            temporary.path().join("guest_token"),
            b"fixture-token-1234567890",
        )
        .expect("write fixture token");
        let (api_url, requests, worker) = fixture_server(vec![
            FixtureResponse::json(
                r#"{"status":"ok","apiVersion":1,"authentication":"bearer","capabilities":["apps-query-v1"]}"#,
            ),
            FixtureResponse::json(
                r#"[{"Name":"Studio Pro 11.12.2","Path":"C:\\Program Files\\Mendix\\11.12.2\\modeler\\StudioPro.exe","Source":"filesystem"}]"#,
            ),
        ]);

        let versions = fetch_installed_versions_at(&config(&compose), &api_url, Instant::now())
            .await
            .expect("projected discovery succeeds");
        worker.join().expect("fixture server finishes");

        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "11.12.2");
        let requests = requests.lock().expect("fixture requests");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /health HTTP/1.1"));
        assert!(!requests[0].to_ascii_lowercase().contains("authorization:"));
        let projected = requests[1].to_ascii_lowercase();
        assert!(projected.starts_with("get /apps?"));
        for query in [
            "includeicons=false",
            "pathprefix=",
            "pathsuffix=",
            "fields=name%2cpath%2csource",
            "limit=128",
        ] {
            assert!(
                projected.contains(query),
                "missing query {query}: {projected}"
            );
        }
        assert!(projected.contains("authorization: bearer fixture-token-1234567890"));
    }

    #[tokio::test]
    async fn falls_back_only_when_the_capability_is_absent() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        let (api_url, requests, worker) = fixture_server(vec![
            FixtureResponse::json(r#"{"status":"ok"}"#),
            FixtureResponse::json(
                r#"[{"Name":"Studio Pro 10.24.3","Path":"C:\\Program Files\\Mendix\\10.24.3.12345\\modeler\\StudioPro.exe","Args":"","Icon":"large-icon","Source":"startmenu"}]"#,
            ),
        ]);

        let versions = fetch_installed_versions_at(&config(&compose), &api_url, Instant::now())
            .await
            .expect("legacy discovery succeeds");
        worker.join().expect("fixture server finishes");

        assert_eq!(versions[0].version, "10.24.3");
        let requests = requests.lock().expect("fixture requests");
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET /apps HTTP/1.1"));
        assert!(!requests[1].to_ascii_lowercase().contains("authorization:"));
    }

    #[tokio::test]
    async fn does_not_downgrade_an_advertised_capability_after_a_malformed_response() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        let (api_url, requests, worker) = fixture_server(vec![
            FixtureResponse::json(r#"{"status":"ok","capabilities":["apps-query-v1"]}"#),
            FixtureResponse::json(
                r#"[{"Name":"Studio Pro","Path":"C:\\Program Files\\Mendix\\11.12.2\\modeler\\StudioPro.exe"}]"#,
            ),
        ]);

        let result = fetch_installed_versions_at(&config(&compose), &api_url, Instant::now()).await;
        worker.join().expect("fixture server finishes");

        assert!(result.is_err());
        let requests = requests.lock().expect("fixture requests");
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET /apps?"));
    }

    #[tokio::test]
    async fn rejects_oversized_projected_responses_before_reading_the_body() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        let (api_url, requests, worker) = fixture_server(vec![
            FixtureResponse::json(r#"{"status":"ok","capabilities":["apps-query-v1"]}"#),
            FixtureResponse {
                status: 200,
                body: "",
                content_length: Some(MAX_PROJECTED_APPS_RESPONSE_BYTES + 1),
            },
        ]);

        let result = fetch_installed_versions_at(&config(&compose), &api_url, Instant::now()).await;
        worker.join().expect("fixture server finishes");
        assert!(result.is_err());
        let requests = requests.lock().expect("fixture requests");
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("GET /apps?"));
    }

    #[tokio::test]
    async fn supports_authenticated_pre_capability_guests_and_fails_closed_on_rejection() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        std::fs::write(
            temporary.path().join("guest_token"),
            b"fixture-token-1234567890",
        )
        .expect("write fixture token");
        let (api_url, requests, worker) = fixture_server(vec![
            FixtureResponse::json(r#"{"status":"ok"}"#),
            FixtureResponse::status(401),
        ]);

        let result = fetch_installed_versions_at(&config(&compose), &api_url, Instant::now()).await;
        worker.join().expect("fixture server finishes");

        assert!(result.is_err());
        let requests = requests.lock().expect("fixture requests");
        assert!(requests[1]
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-token-1234567890"));
    }

    #[tokio::test]
    async fn stops_before_apps_when_advertised_bearer_authentication_has_no_token() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        let (api_url, requests, worker) = fixture_server(vec![FixtureResponse::json(
            r#"{"status":"ok","authentication":"bearer","capabilities":["apps-query-v1"]}"#,
        )]);

        let result = fetch_installed_versions_at(&config(&compose), &api_url, Instant::now()).await;
        worker.join().expect("fixture server finishes");

        assert!(result.is_err());
        assert_eq!(requests.lock().expect("fixture requests").len(), 1);
    }

    #[test]
    fn bounds_app_counts_and_fields() {
        initialize_i18n();
        let app = WinApp {
            name: "Studio Pro".into(),
            path: r"C:\Program Files\Mendix\11.12.2\modeler\StudioPro.exe".into(),
            source: "fixture".into(),
        };
        assert!(validate_apps(&vec![app.clone(); MAX_PROJECTED_APPS], MAX_PROJECTED_APPS).is_ok());
        assert!(validate_apps(
            &vec![app.clone(); MAX_PROJECTED_APPS + 1],
            MAX_PROJECTED_APPS
        )
        .is_err());
        let mut invalid = app;
        invalid.path.push('\n');
        assert!(validate_apps(&[invalid], MAX_PROJECTED_APPS).is_err());
    }

    #[test]
    fn marks_guest_authorization_as_sensitive() {
        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        std::fs::write(
            temporary.path().join("guest_token"),
            b"fixture-token-1234567890",
        )
        .expect("write fixture token");

        let authorization = load_guest_authorization(&config(&compose))
            .expect("load fixture token")
            .expect("authorization header");
        assert!(authorization.is_sensitive());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_and_writable_guest_tokens() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        initialize_i18n();
        let temporary = tempfile::tempdir().expect("temporary WinBoat directory");
        let compose = temporary.path().join("docker-compose.yml");
        let target = temporary.path().join("token-target");
        let token = temporary.path().join("guest_token");
        std::fs::write(&target, b"fixture-token-1234567890").expect("write token target");
        symlink(&target, &token).expect("symlink token");
        assert!(load_guest_authorization(&config(&compose)).is_err());

        std::fs::remove_file(&token).expect("remove token symlink");
        std::fs::write(&token, b"fixture-token-1234567890").expect("write direct token");
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o666))
            .expect("make token unsafe");
        assert!(load_guest_authorization(&config(&compose)).is_err());
    }
}
