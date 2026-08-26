use super::container::{guest_is_online_at, http_client};
use crate::config::runtime_host_port_async;
use crate::models::{AppConfig, StudioVersion, WinApp};
use futures_util::StreamExt;
use regex::Regex;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const GUEST_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const MAX_APPS_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

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
    if !guest_is_online_at(&api_url).await {
        return Err(crate::tr!("error-guest-offline"));
    }
    let health_elapsed = started_at.elapsed();
    let client = http_client(Duration::from_secs(GUEST_REQUEST_TIMEOUT_SECONDS))?;
    let request_started_at = Instant::now();
    let response = client
        .get(format!("{api_url}/apps"))
        .send()
        .await
        .map_err(|error| crate::tr!("error-windows-apps-fetch", error = error))?
        .error_for_status()
        .map_err(|error| crate::tr!("error-windows-apps-response", error = error))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_APPS_RESPONSE_BYTES as u64)
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
        if payload.len().saturating_add(chunk.len()) > MAX_APPS_RESPONSE_BYTES {
            return Err(crate::tr!(
                "error-windows-apps-parse",
                error = "response exceeds the safe size limit"
            ));
        }
        payload.extend_from_slice(&chunk);
    }
    let request_elapsed = request_started_at.elapsed();
    let parse_started_at = Instant::now();
    let apps = serde_json::from_slice::<Vec<WinApp>>(&payload)
        .map_err(|error| crate::tr!("error-windows-apps-parse", error = error))?;
    let app_count = apps.len();
    let versions = parse_studio_versions(apps, &config.mendix_install_root);
    trace_guest_apps(
        health_elapsed,
        request_elapsed,
        parse_started_at.elapsed(),
        payload.len(),
        app_count,
        versions.len(),
    );
    Ok(versions)
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
        "[studio-overview] guest-apps health_ms={} request_ms={} parse_ms={} payload_bytes={payload_bytes} app_count={app_count} studio_count={studio_count}",
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
