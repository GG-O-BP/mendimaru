use crate::config::{
    compose_shared_directory, path_exists_or_binary, resolved_api_url, resolved_winboat_executable,
};
use crate::models::{AppConfig, ContainerRuntime, ContainerStatus, EnvironmentStatus};
use serde::Deserialize;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

pub async fn environment_status(config: &AppConfig) -> EnvironmentStatus {
    let winboat_available = resolved_winboat_executable(config).is_some();
    let compose_available = Path::new(&config.compose_file).is_file();
    let runtime_available = path_exists_or_binary(config.container_runtime.as_str());
    let freerdp_available = path_exists_or_binary(&config.freerdp_binary);
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

    EnvironmentStatus {
        winboat_available,
        winboat_initialized,
        setup_pending: config.winboat_setup_pending,
        compose_available,
        runtime_available,
        freerdp_available,
        shared_directory_available,
        shared_mount_matches,
        container_status,
        guest_online,
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

#[cfg(test)]
mod tests {
    use super::parse_container_inspection;
    use crate::models::ContainerStatus;

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
}
