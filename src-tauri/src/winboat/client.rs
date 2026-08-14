use super::container::{guest_is_online, http_client};
use crate::config::resolved_api_url;
use crate::models::{AppConfig, StudioVersion, WinApp};
use regex::Regex;
use std::collections::BTreeMap;
use std::time::Duration;

const GUEST_REQUEST_TIMEOUT_SECONDS: u64 = 30;

pub async fn installed_versions(config: &AppConfig) -> Result<Vec<StudioVersion>, String> {
    if !guest_is_online(config).await {
        return Err(crate::tr!("error-guest-offline"));
    }
    let client = http_client(Duration::from_secs(GUEST_REQUEST_TIMEOUT_SECONDS))?;
    let api_url = resolved_api_url(config);
    let response = client
        .get(format!("{api_url}/apps"))
        .send()
        .await
        .map_err(|error| crate::tr!("error-windows-apps-fetch", error = error))?
        .error_for_status()
        .map_err(|error| crate::tr!("error-windows-apps-response", error = error))?;
    let apps = response
        .json::<Vec<WinApp>>()
        .await
        .map_err(|error| crate::tr!("error-windows-apps-parse", error = error))?;
    Ok(parse_studio_versions(apps, &config.mendix_install_root))
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
