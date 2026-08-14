use crate::config::{
    compose_file_is_valid, compose_shared_directory, path_exists_or_binary, resolved_api_url,
    resolved_rdp_port, resolved_winboat_executable,
};
use crate::models::{
    AppConfig, ContainerRuntime, ContainerStatus, EnvironmentDiagnostic,
    EnvironmentDiagnosticAction, EnvironmentDiagnosticId, EnvironmentDiagnosticStatus,
    EnvironmentStatus,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub async fn environment_status(config: &AppConfig) -> EnvironmentStatus {
    let winboat_available = resolved_winboat_executable(config).is_some();
    let compose_available = Path::new(&config.compose_file).is_file();
    let compose_valid = compose_available && compose_file_is_valid(&config.compose_file);
    let runtime = probe_container_runtime(config.container_runtime.as_str());
    let freerdp = probe_tool(&config.freerdp_binary, &["/version"]);
    let browser = crate::marketplace::browser_executable()
        .map(|executable| probe_tool(&executable, &["--version"]))
        .unwrap_or_default();
    let shared_directory_available = Path::new(&config.shared_directory).is_dir();
    let inspection = inspect_container(config);
    let compose_shared = compose_shared_directory(&config.compose_file);
    let current_shared = if inspection.status.exists() {
        inspection.shared_directory.as_deref()
    } else {
        compose_shared.as_deref()
    };
    let shared_mount_matches = compose_available
        && current_shared
            .is_some_and(|current| paths_refer_to_same_location(current, &config.shared_directory));
    let container_status = inspection.status;
    let winboat_initialized = compose_available && container_status.exists();
    let guest_online =
        winboat_initialized && container_status.is_running() && guest_is_online(config).await;
    let rdp_reachable = container_status.is_running()
        && tcp_endpoint_reachable(&config.rdp_host, resolved_rdp_port(config));
    let state = LinuxDiagnosticState {
        winboat_available,
        compose_available,
        compose_valid,
        runtime,
        freerdp,
        shared_directory_available,
        shared_mount_matches,
        container_status,
        guest_online,
        rdp_reachable,
        browser,
    };
    let ready = state.ready();
    let diagnostics = build_linux_diagnostics(&state);

    EnvironmentStatus {
        platform: crate::platform::capabilities(),
        ready,
        winboat_available,
        winboat_initialized,
        setup_pending: config.winboat_setup_pending,
        compose_available,
        runtime_available: state.runtime.usable,
        freerdp_available: state.freerdp.usable,
        shared_directory_available,
        shared_mount_matches,
        container_status,
        guest_online,
        diagnostics,
    }
}

#[derive(Debug, Clone, Default)]
struct ToolProbe {
    available: bool,
    usable: bool,
    version: Option<String>,
}

#[derive(Debug, Clone)]
struct LinuxDiagnosticState {
    winboat_available: bool,
    compose_available: bool,
    compose_valid: bool,
    runtime: ToolProbe,
    freerdp: ToolProbe,
    shared_directory_available: bool,
    shared_mount_matches: bool,
    container_status: ContainerStatus,
    guest_online: bool,
    rdp_reachable: bool,
    browser: ToolProbe,
}

impl LinuxDiagnosticState {
    fn ready(&self) -> bool {
        self.winboat_available
            && self.compose_valid
            && self.runtime.usable
            && self.freerdp.usable
            && self.shared_directory_available
            && self.shared_mount_matches
            && self.container_status.is_running()
            && self.guest_online
            && self.rdp_reachable
    }
}

fn build_linux_diagnostics(state: &LinuxDiagnosticState) -> Vec<EnvironmentDiagnostic> {
    use EnvironmentDiagnosticAction::{OpenSettings, OpenWinboat, Redetect, StartWinboat};
    use EnvironmentDiagnosticId::{
        Compose, Container, ContainerRuntime, Freerdp, GuestApi, MarketplaceBrowser, Rdp,
        SharedDirectory, SharedMount, Winboat,
    };
    use EnvironmentDiagnosticStatus::{Failure, Success, Warning};

    vec![
        diagnostic(
            Winboat,
            if state.winboat_available {
                Success
            } else {
                Failure
            },
            None,
            (!state.winboat_available).then_some(Redetect),
        ),
        diagnostic(
            Compose,
            if state.compose_valid {
                Success
            } else {
                Failure
            },
            state.compose_available.then(|| {
                if state.compose_valid {
                    "valid"
                } else {
                    "invalid"
                }
                .to_string()
            }),
            (!state.compose_valid).then_some(if state.compose_available {
                OpenSettings
            } else {
                Redetect
            }),
        ),
        diagnostic(
            ContainerRuntime,
            if state.runtime.usable {
                Success
            } else {
                Failure
            },
            observed_tool(&state.runtime),
            (!state.runtime.usable).then_some(OpenSettings),
        ),
        diagnostic(
            Freerdp,
            if state.freerdp.usable {
                Success
            } else {
                Failure
            },
            observed_tool(&state.freerdp),
            (!state.freerdp.usable).then_some(Redetect),
        ),
        diagnostic(
            SharedDirectory,
            if state.shared_directory_available {
                Success
            } else {
                Failure
            },
            None,
            (!state.shared_directory_available).then_some(OpenSettings),
        ),
        diagnostic(
            SharedMount,
            if state.shared_mount_matches {
                Success
            } else if state.compose_valid && state.shared_directory_available {
                Failure
            } else {
                Warning
            },
            None,
            (!state.shared_mount_matches).then_some(OpenSettings),
        ),
        diagnostic(
            Container,
            if state.container_status.is_running() {
                Success
            } else if matches!(
                state.container_status,
                ContainerStatus::Dead | ContainerStatus::Unknown
            ) {
                Failure
            } else {
                Warning
            },
            Some(container_status_name(state.container_status).to_string()),
            (!state.container_status.is_running()).then_some(StartWinboat),
        ),
        diagnostic(
            GuestApi,
            if state.guest_online {
                Success
            } else if state.container_status.is_running() {
                Failure
            } else {
                Warning
            },
            None,
            (!state.guest_online).then_some(if state.container_status.is_running() {
                OpenWinboat
            } else {
                StartWinboat
            }),
        ),
        diagnostic(
            Rdp,
            if state.rdp_reachable {
                Success
            } else if state.container_status.is_running() {
                Failure
            } else {
                Warning
            },
            None,
            (!state.rdp_reachable).then_some(if state.container_status.is_running() {
                OpenWinboat
            } else {
                StartWinboat
            }),
        ),
        diagnostic(
            MarketplaceBrowser,
            if state.browser.usable {
                Success
            } else {
                Warning
            },
            observed_tool(&state.browser),
            (!state.browser.usable).then_some(Redetect),
        ),
    ]
}

fn observed_tool(probe: &ToolProbe) -> Option<String> {
    probe
        .version
        .clone()
        .or_else(|| probe.available.then(|| "detected-but-unusable".to_string()))
}

fn diagnostic(
    id: EnvironmentDiagnosticId,
    status: EnvironmentDiagnosticStatus,
    observed: Option<String>,
    action: Option<EnvironmentDiagnosticAction>,
) -> EnvironmentDiagnostic {
    EnvironmentDiagnostic {
        id,
        status,
        observed,
        action,
    }
}

fn probe_container_runtime(executable: &str) -> ToolProbe {
    if !path_exists_or_binary(executable) {
        return ToolProbe::default();
    }
    probe_tool(executable, &["info", "--format", "{{.ServerVersion}}"])
}

fn probe_tool(executable: &str, arguments: &[&str]) -> ToolProbe {
    if !path_exists_or_binary(executable) {
        return ToolProbe::default();
    }
    let Some(output) = command_output_with_timeout(executable, arguments, Duration::from_secs(2))
    else {
        return ToolProbe {
            available: true,
            ..ToolProbe::default()
        };
    };
    let raw = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    ToolProbe {
        available: true,
        usable: output.status.success(),
        version: output
            .status
            .success()
            .then(|| extract_version(raw))
            .flatten(),
    }
}

fn command_output_with_timeout(
    executable: &str,
    arguments: &[&str],
    timeout: Duration,
) -> Option<Output> {
    let mut child = Command::new(executable)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn extract_version(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '.' | '-' | '+' | '_')
            })
        })
        .find(|token| {
            token
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
                && token.contains('.')
                && token.len() <= 64
        })
        .map(ToString::to_string)
}

fn tcp_endpoint_reachable(host: &str, port: u16) -> bool {
    if !is_loopback_host(host) || port == 0 {
        return false;
    }
    (host, port)
        .to_socket_addrs()
        .ok()
        .into_iter()
        .flatten()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok())
}

const fn container_status_name(status: ContainerStatus) -> &'static str {
    match status {
        ContainerStatus::Created => "created",
        ContainerStatus::Restarting => "restarting",
        ContainerStatus::Running => "running",
        ContainerStatus::Removing => "removing",
        ContainerStatus::Paused => "paused",
        ContainerStatus::Exited => "exited",
        ContainerStatus::Dead => "dead",
        ContainerStatus::NotFound => "not-found",
        ContainerStatus::Unknown => "unknown",
    }
}

pub async fn start_container(config: &AppConfig) -> Result<ContainerStatus, String> {
    if !Path::new(&config.compose_file).is_file() {
        return Err(crate::tr!(
            "error-compose-file-not-found",
            path = &config.compose_file
        ));
    }
    let status = inspect_container_status(config);
    if status.is_running() {
        return Ok(status);
    }

    if status.exists() {
        let output = Command::new(config.container_runtime.as_str())
            .arg("start")
            .arg(&config.container_name)
            .output()
            .map_err(|error| crate::tr!("error-container-start", error = error))?;
        ensure_success(output, &crate::tr!("operation-container-start"))?;
    } else {
        compose_up(config, false).await?;
    }
    Ok(inspect_container_status(config))
}

pub async fn recreate_container(config: &AppConfig) -> Result<(), String> {
    compose_up(config, true).await
}

pub fn open_winboat(config: &AppConfig) -> Result<(), String> {
    let executable = resolved_winboat_executable(config)
        .ok_or_else(|| crate::tr!("error-winboat-executable-not-found"))?;
    Command::new(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| crate::tr!("error-winboat-open", error = error))
}

async fn compose_up(config: &AppConfig, force_recreate: bool) -> Result<(), String> {
    let mut command;
    if config.container_runtime == ContainerRuntime::Podman
        && path_exists_or_binary("podman-compose")
    {
        command = Command::new("podman-compose");
    } else {
        command = Command::new(config.container_runtime.as_str());
        command.arg("compose");
    }
    command
        .arg("-f")
        .arg(&config.compose_file)
        .arg("up")
        .arg("-d");
    if force_recreate {
        command.arg("--force-recreate");
    }
    let output = command
        .output()
        .map_err(|error| crate::tr!("error-compose-run", error = error))?;
    ensure_success(output, &crate::tr!("operation-compose-apply"))
}

pub(super) async fn ensure_guest_online(config: &AppConfig) -> Result<(), String> {
    if guest_is_online(config).await {
        return Ok(());
    }
    start_container(config).await?;
    let timeout = Duration::from_secs(config.startup_timeout_seconds);
    let started = tokio::time::Instant::now();
    while started.elapsed() < timeout {
        if guest_is_online(config).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(crate::tr!(
        "error-guest-timeout",
        seconds = crate::i18n::format_number(config.startup_timeout_seconds)
    ))
}

pub(super) fn ensure_private_operation_transport(config: &AppConfig) -> Result<(), String> {
    if !is_loopback_host(&config.rdp_host) {
        return Err(crate::tr!(
            "error-winboat-transport-public",
            endpoint = &config.rdp_host
        ));
    }
    let api_url = reqwest::Url::parse(&resolved_api_url(config))
        .map_err(|error| crate::tr!("error-winboat-transport-inspect", error = error))?;
    if !api_url.host_str().is_some_and(is_loopback_host) {
        return Err(crate::tr!(
            "error-winboat-transport-public",
            endpoint = api_url.as_str()
        ));
    }

    let inspection = runtime_inspection(config)?;
    for guest_port in ["3389/tcp", "7148/tcp"] {
        let bindings = inspection
            .network_settings
            .ports
            .get(guest_port)
            .and_then(Option::as_ref)
            .filter(|bindings| !bindings.is_empty())
            .ok_or_else(|| {
                crate::tr!("error-winboat-transport-binding-missing", port = guest_port)
            })?;
        if bindings
            .iter()
            .any(|binding| !is_loopback_host(&binding.host_ip))
        {
            return Err(crate::tr!(
                "error-winboat-transport-public",
                endpoint = format!("{} ({guest_port})", bindings[0].host_ip)
            ));
        }
    }
    Ok(())
}

pub async fn guest_is_online(config: &AppConfig) -> bool {
    let Ok(client) = http_client(Duration::from_secs(2)) else {
        return false;
    };
    let api_url = resolved_api_url(config);
    client
        .get(format!("{api_url}/health"))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

pub(super) fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("mendimaru/0.1 (WinBoat Studio Pro manager)")
        .build()
        .map_err(|error| crate::tr!("error-http-client-create", error = error))
}

fn inspect_container_status(config: &AppConfig) -> ContainerStatus {
    inspect_container(config).status
}

#[derive(Debug)]
struct ContainerInspection {
    status: ContainerStatus,
    shared_directory: Option<String>,
}

impl ContainerInspection {
    fn not_found() -> Self {
        Self {
            status: ContainerStatus::NotFound,
            shared_directory: None,
        }
    }

    fn unknown() -> Self {
        Self {
            status: ContainerStatus::Unknown,
            shared_directory: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeContainerInspection {
    state: RuntimeContainerState,
    #[serde(default)]
    mounts: Vec<RuntimeContainerMount>,
    #[serde(default)]
    network_settings: RuntimeNetworkSettings,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeNetworkSettings {
    #[serde(default)]
    ports: BTreeMap<String, Option<Vec<RuntimePortBinding>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimePortBinding {
    host_ip: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeContainerState {
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeContainerMount {
    source: String,
    destination: String,
}

fn inspect_container(config: &AppConfig) -> ContainerInspection {
    let output = Command::new(config.container_runtime.as_str())
        .arg("inspect")
        .arg(&config.container_name)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_container_inspection(&output.stdout).unwrap_or_else(ContainerInspection::unknown)
        }
        _ => ContainerInspection::not_found(),
    }
}

fn runtime_inspection(config: &AppConfig) -> Result<RuntimeContainerInspection, String> {
    let output = Command::new(config.container_runtime.as_str())
        .arg("inspect")
        .arg(&config.container_name)
        .output()
        .map_err(|error| crate::tr!("error-winboat-transport-inspect", error = error))?;
    if !output.status.success() {
        return Err(crate::tr!(
            "error-winboat-transport-inspect",
            error = output.status
        ));
    }
    serde_json::from_slice::<Vec<RuntimeContainerInspection>>(&output.stdout)
        .map_err(|error| crate::tr!("error-winboat-transport-inspect", error = error))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            crate::tr!(
                "error-winboat-transport-inspect",
                error = "the runtime returned no container"
            )
        })
}

fn parse_container_inspection(output: &[u8]) -> Option<ContainerInspection> {
    let containers = serde_json::from_slice::<Vec<RuntimeContainerInspection>>(output).ok()?;
    let container = containers.into_iter().next()?;
    let shared_directory = container
        .mounts
        .into_iter()
        .find(|mount| mount.destination == "/shared")
        .map(|mount| mount.source);
    Some(ContainerInspection {
        status: ContainerStatus::from_runtime(&container.state.status),
        shared_directory,
    })
}

fn ensure_success(output: Output, operation: &str) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(crate::tr!(
            "error-operation-status",
            operation = operation,
            status = output.status
        ))
    } else {
        Err(crate::tr!(
            "error-operation-detail",
            operation = operation,
            detail = stderr
        ))
    }
}

fn paths_refer_to_same_location(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left_path == right_path,
    }
}

fn is_loopback_host(value: &str) -> bool {
    value.eq_ignore_ascii_case("localhost")
        || value
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::{
        build_linux_diagnostics, environment_status, extract_version, is_loopback_host,
        parse_container_inspection, LinuxDiagnosticState, RuntimeContainerInspection, ToolProbe,
    };
    use crate::models::{
        ContainerStatus, EnvironmentDiagnosticAction, EnvironmentDiagnosticId,
        EnvironmentDiagnosticStatus,
    };

    #[test]
    fn parses_status_and_active_shared_mount_from_runtime_inspection() {
        let inspection = parse_container_inspection(
            br#"[{"State":{"Status":"running"},"Mounts":[{"Source":"/home/dev/Mendix","Destination":"/shared"},{"Source":"volume","Destination":"/storage"}]}]"#,
        )
        .expect("runtime inspection parses");

        assert_eq!(inspection.status, ContainerStatus::Running);
        assert_eq!(
            inspection.shared_directory.as_deref(),
            Some("/home/dev/Mendix")
        );
    }

    #[test]
    fn accepts_only_explicit_loopback_transport_bindings() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.50"));

        let inspection = serde_json::from_str::<Vec<RuntimeContainerInspection>>(
            r#"[{"State":{"Status":"running"},"Mounts":[],"NetworkSettings":{"Ports":{"3389/tcp":[{"HostIp":"127.0.0.1","HostPort":"47300"}],"7148/tcp":[{"HostIp":"::1","HostPort":"47280"}]}}}]"#,
        )
        .expect("port bindings parse");
        assert_eq!(inspection[0].network_settings.ports.len(), 2);
    }

    fn healthy_state() -> LinuxDiagnosticState {
        LinuxDiagnosticState {
            winboat_available: true,
            compose_available: true,
            compose_valid: true,
            runtime: ToolProbe {
                available: true,
                usable: true,
                version: Some("29.7.2".to_string()),
            },
            freerdp: ToolProbe {
                available: true,
                usable: true,
                version: Some("3.30.0".to_string()),
            },
            shared_directory_available: true,
            shared_mount_matches: true,
            container_status: ContainerStatus::Running,
            guest_online: true,
            rdp_reachable: true,
            browser: ToolProbe {
                available: true,
                usable: true,
                version: Some("151.0.7922.137".to_string()),
            },
        }
    }

    #[test]
    fn healthy_environment_has_independent_successful_checks() {
        let state = healthy_state();
        let diagnostics = build_linux_diagnostics(&state);

        assert!(state.ready());
        assert_eq!(diagnostics.len(), 10);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.status == EnvironmentDiagnosticStatus::Success));
        assert_eq!(
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::ContainerRuntime)
                .and_then(|diagnostic| diagnostic.observed.as_deref()),
            Some("29.7.2")
        );
    }

    #[test]
    fn partial_failures_keep_recovery_actions_and_unrelated_successes() {
        let mut state = healthy_state();
        state.compose_valid = false;
        state.guest_online = false;
        state.browser = ToolProbe::default();
        let diagnostics = build_linux_diagnostics(&state);

        assert!(!state.ready());
        let compose = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::Compose)
            .expect("compose check");
        assert_eq!(compose.status, EnvironmentDiagnosticStatus::Failure);
        assert_eq!(
            compose.action,
            Some(EnvironmentDiagnosticAction::OpenSettings)
        );
        let guest = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestApi)
            .expect("guest check");
        assert_eq!(guest.status, EnvironmentDiagnosticStatus::Failure);
        assert_eq!(guest.action, Some(EnvironmentDiagnosticAction::OpenWinboat));
        let browser = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::MarketplaceBrowser)
            .expect("browser check");
        assert_eq!(browser.status, EnvironmentDiagnosticStatus::Warning);
        assert_eq!(browser.action, Some(EnvironmentDiagnosticAction::Redetect));
        assert_eq!(
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::Freerdp)
                .map(|diagnostic| diagnostic.status),
            Some(EnvironmentDiagnosticStatus::Success)
        );
    }

    #[test]
    fn version_output_is_reduced_to_a_safe_component_version() {
        assert_eq!(
            extract_version(b"Google Chrome 151.0.7922.137\n"),
            Some("151.0.7922.137".to_string())
        );
        assert_eq!(
            extract_version(b"This is FreeRDP version 3.30.0 (build)"),
            Some("3.30.0".to_string())
        );
        assert_eq!(extract_version(b"password=secret"), None);
    }

    #[test]
    #[ignore = "checks the real Linux host, WinBoat guest, RDP endpoint, and Marketplace browser"]
    fn live_e2e_reports_every_linux_dependency_without_secrets() {
        use crate::models::{environment_diagnostic_report, EnvironmentDiagnosticId};
        use std::collections::BTreeSet;

        crate::i18n::initialize("en-US").expect("English localization initializes");
        let config = crate::config::detect_config().expect("live configuration is detected");
        let status = tauri::async_runtime::block_on(environment_status(&config));
        assert!(status.ready, "live environment diagnostics: {status:#?}");
        assert_eq!(status.diagnostics.len(), 10);
        let ids = status
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), status.diagnostics.len());
        for expected in [
            EnvironmentDiagnosticId::Winboat,
            EnvironmentDiagnosticId::Compose,
            EnvironmentDiagnosticId::ContainerRuntime,
            EnvironmentDiagnosticId::Freerdp,
            EnvironmentDiagnosticId::SharedDirectory,
            EnvironmentDiagnosticId::SharedMount,
            EnvironmentDiagnosticId::Container,
            EnvironmentDiagnosticId::GuestApi,
            EnvironmentDiagnosticId::Rdp,
            EnvironmentDiagnosticId::MarketplaceBrowser,
        ] {
            assert!(ids.contains(&expected), "missing {expected:?}");
        }
        assert!(status
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.status == EnvironmentDiagnosticStatus::Success));

        let report = environment_diagnostic_report(&status).expect("report serializes");
        for sensitive in [
            &config.shared_directory,
            &config.compose_file,
            &config.winboat_executable,
            &config.windows_shared_directory,
        ] {
            assert!(
                sensitive.is_empty() || !report.contains(sensitive),
                "report leaked configured path"
            );
        }
        assert!(!report.to_ascii_lowercase().contains("password"));
        assert!(!report.to_ascii_lowercase().contains("token"));
    }
}
