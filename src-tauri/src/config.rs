mod compose;
mod store;
mod validation;

use crate::models::{AppConfig, ContainerRuntime};
use crate::process::{self, CommandFailure, CommandPolicy};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tokio::process::Command as TokioCommand;

#[cfg(all(test, target_os = "linux"))]
pub(crate) use compose::snapshot_file;
pub(crate) use compose::{
    apply_shared_mount, plan_shared_mount, restore_file_if_revision, verify_plan_revision,
    winboat_compose_service_name, AppliedSharedMount, ComposeError, ComposeErrorKind,
};
pub use compose::{compose_file_is_valid, compose_shared_directory};
#[cfg(target_os = "linux")]
pub(crate) use compose::{
    ensure_runtime_port_mapping, prepare_runtime_compose_baseline, restore_file,
    runtime_port_mapping, FileSnapshot, RuntimeComposeBaseline, RuntimePortMapping,
};
pub use store::{load_config, persist_config};
pub(crate) use store::{load_config_from, restore_config, snapshot_config, ConfigSnapshot};
pub(crate) use validation::normalize_and_validate;

const DEFAULT_API_PORT: u16 = 47280;
const DEFAULT_RDP_PORT: u16 = 47300;
const WINBOAT_API_GUEST_PORT: u16 = 7148;
const WINBOAT_RDP_GUEST_PORT: u16 = 3389;

pub fn detect_config() -> Result<AppConfig, String> {
    if crate::platform::is_windows_native() {
        return detect_windows_config();
    }
    let home = home_directory()?;
    let data_home = data_home_directory(&home);
    let compose_candidates = compose_candidates(&home, &data_home);
    let compose_file = compose_candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .unwrap_or_else(|| compose_candidates[0].clone());

    let mut config = AppConfig {
        language_preference: "system".to_string(),
        winboat_setup_pending: false,
        winboat_executable: discover_winboat_executable_with_home(&home).unwrap_or_else(|| {
            home.join(".local/bin/winboat")
                .to_string_lossy()
                .to_string()
        }),
        compose_file: compose_file.to_string_lossy().to_string(),
        container_runtime: runtime_for_compose(&compose_file),
        container_name: "WinBoat".to_string(),
        api_url: format!("http://127.0.0.1:{DEFAULT_API_PORT}"),
        rdp_host: "127.0.0.1".to_string(),
        rdp_port: DEFAULT_RDP_PORT,
        shared_directory: home.to_string_lossy().to_string(),
        windows_shared_directory: r"\\host.lan\Data".to_string(),
        freerdp_binary: find_binary(&["xfreerdp3", "xfreerdp"])
            .or_else(|| {
                [
                    "/usr/bin/xfreerdp3",
                    "/usr/bin/xfreerdp",
                    "/usr/local/bin/xfreerdp",
                ]
                .into_iter()
                .find(|candidate| Path::new(candidate).is_file())
                .map(str::to_string)
            })
            .unwrap_or_else(|| "/usr/bin/xfreerdp3".to_string()),
        mendix_install_root: r"C:\Program Files\Mendix".to_string(),
        mendix_data_root: r"C:\ProgramData\Mendix".to_string(),
        windows_studio_paths: Vec::new(),
        startup_timeout_seconds: 180,
    };

    if compose_file.is_file() {
        if let Ok(compose) = compose::read_compose(&compose_file) {
            compose::apply_compose_detection(&mut config, &compose);
        }
    }
    apply_runtime_port_detection(&mut config);
    Ok(config)
}

fn detect_windows_config() -> Result<AppConfig, String> {
    #[cfg(not(feature = "e2e"))]
    let home = home_directory()?;
    #[cfg(feature = "e2e")]
    let default_workspace = crate::e2e::directory("workspace")?;
    #[cfg(not(feature = "e2e"))]
    let default_workspace = home.join("Mendix");
    let program_files = env::var_os("ProgramW6432")
        .or_else(|| env::var_os("ProgramFiles"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let program_data = env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));

    Ok(AppConfig {
        language_preference: "system".to_string(),
        winboat_setup_pending: false,
        winboat_executable: String::new(),
        compose_file: String::new(),
        container_runtime: ContainerRuntime::Docker,
        container_name: String::new(),
        api_url: String::new(),
        rdp_host: String::new(),
        rdp_port: 0,
        shared_directory: default_workspace.to_string_lossy().to_string(),
        windows_shared_directory: String::new(),
        freerdp_binary: String::new(),
        mendix_install_root: program_files.join("Mendix").to_string_lossy().to_string(),
        mendix_data_root: program_data.join("Mendix").to_string_lossy().to_string(),
        windows_studio_paths: Vec::new(),
        startup_timeout_seconds: 180,
    })
}

pub(crate) fn migrate_legacy_windows_workspace(config: &mut AppConfig) -> bool {
    if !crate::platform::is_windows_native() {
        return false;
    }
    let Ok(home) = home_directory() else {
        return false;
    };
    let Some(workspace) =
        migrated_windows_workspace(Path::new(&config.shared_directory), &home, true)
    else {
        return false;
    };
    config.shared_directory = workspace.to_string_lossy().to_string();
    true
}

fn migrated_windows_workspace(
    configured: &Path,
    home: &Path,
    windows_native: bool,
) -> Option<PathBuf> {
    if !windows_native || normalized_path(configured) != normalized_path(home) {
        return None;
    }
    Some(home.join("Mendix"))
}

fn normalized_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

pub fn path_exists_or_binary(path: &str) -> bool {
    let expanded = expand_home(path);
    Path::new(&expanded).is_file() || find_binary(&[path]).is_some()
}

pub fn resolved_winboat_executable(config: &AppConfig) -> Option<String> {
    let configured = expand_home(config.winboat_executable.trim());
    if Path::new(&configured).is_file() {
        return Some(configured);
    }
    discover_winboat_executable()
}

pub fn runtime_host_port(config: &AppConfig, guest_port: u16, protocol: &str) -> Option<u16> {
    runtime_host_port_sync(
        config.container_runtime.as_str(),
        &config.container_name,
        guest_port,
        protocol,
        CommandPolicy::STATUS,
    )
    .ok()
    .flatten()
}

pub(crate) async fn runtime_host_port_async(
    config: &AppConfig,
    guest_port: u16,
    protocol: &str,
) -> Result<Option<u16>, CommandFailure> {
    runtime_host_port_for(
        config.container_runtime.as_str(),
        &config.container_name,
        guest_port,
        protocol,
        CommandPolicy::STATUS,
    )
    .await
}

async fn runtime_host_port_for(
    runtime: &str,
    container_name: &str,
    guest_port: u16,
    protocol: &str,
    policy: CommandPolicy,
) -> Result<Option<u16>, CommandFailure> {
    let private_port = format!("{guest_port}/{protocol}");
    let mut port = TokioCommand::new(runtime);
    port.arg("port").arg(container_name).arg(&private_port);
    let output = process::output(port, policy, None, "container port lookup").await?;
    if output.status.success() {
        if let Some(port) = parse_runtime_port_output(&String::from_utf8_lossy(&output.stdout)) {
            return Ok(Some(port));
        }
    }

    let mut inspect = TokioCommand::new(runtime);
    inspect
        .arg("inspect")
        .arg("--format")
        .arg("{{json .NetworkSettings.Ports}}")
        .arg(container_name);
    let inspect = process::output(inspect, policy, None, "container port inspection").await?;
    if !inspect.status.success() {
        return Ok(None);
    }
    Ok(inspect_host_port(
        &String::from_utf8_lossy(&inspect.stdout),
        &private_port,
    ))
}

fn runtime_host_port_sync(
    runtime: &str,
    container_name: &str,
    guest_port: u16,
    protocol: &str,
    policy: CommandPolicy,
) -> Result<Option<u16>, CommandFailure> {
    let private_port = format!("{guest_port}/{protocol}");
    let mut port = StdCommand::new(runtime);
    port.arg("port").arg(container_name).arg(&private_port);
    let output = process::output_sync(port, policy, None, "container port lookup")?;
    if output.status.success() {
        if let Some(port) = parse_runtime_port_output(&String::from_utf8_lossy(&output.stdout)) {
            return Ok(Some(port));
        }
    }

    let mut inspect = StdCommand::new(runtime);
    inspect
        .arg("inspect")
        .arg("--format")
        .arg("{{json .NetworkSettings.Ports}}")
        .arg(container_name);
    let inspect = process::output_sync(inspect, policy, None, "container port inspection")?;
    if !inspect.status.success() {
        return Ok(None);
    }
    Ok(inspect_host_port(
        &String::from_utf8_lossy(&inspect.stdout),
        &private_port,
    ))
}

pub fn find_binary(names: &[&str]) -> Option<String> {
    let path_value = env::var_os("PATH")?;
    for name in names {
        let candidate_path = Path::new(name);
        if candidate_path.components().count() > 1 && candidate_path.is_file() {
            return Some(candidate_path.to_string_lossy().to_string());
        }
        for directory in env::split_paths(&path_value) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn discover_winboat_executable() -> Option<String> {
    let home = home_directory().ok()?;
    discover_winboat_executable_with_home(&home)
}

fn discover_winboat_executable_with_home(home: &Path) -> Option<String> {
    find_binary(&["winboat", "WinBoat"]).or_else(|| {
        [
            PathBuf::from("/opt/winboat/winboat"),
            PathBuf::from("/usr/local/bin/winboat"),
            PathBuf::from("/usr/bin/winboat"),
            home.join(".local/bin/winboat"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
    })
}

fn data_home_directory(home: &Path) -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
}

fn compose_candidates(home: &Path, data_home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".winboat/docker-compose.yml"),
        home.join(".winboat/podman-compose.yml"),
        data_home.join("winboat-app/docker-compose.yml"),
        data_home.join("winboat-app/podman-compose.yml"),
        home.join(".config/winboat/docker-compose.yml"),
        home.join(".config/winboat/podman-compose.yml"),
    ]
}

fn runtime_for_compose(compose_file: &Path) -> ContainerRuntime {
    if compose_file
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("podman"))
    {
        ContainerRuntime::Podman
    } else {
        ContainerRuntime::Docker
    }
}

pub fn expand_home(value: &str) -> String {
    if value == "~" || value.starts_with("~/") {
        if let Ok(home) = home_directory() {
            return if value == "~" {
                home.to_string_lossy().to_string()
            } else {
                home.join(&value[2..]).to_string_lossy().to_string()
            };
        }
    }
    value.to_string()
}

pub(crate) fn home_directory() -> Result<PathBuf, String> {
    preferred_home_directory(
        env::var_os("HOME"),
        env::var_os("USERPROFILE"),
        crate::platform::is_windows_native(),
    )
    .filter(|path| !path.as_os_str().is_empty())
    .ok_or_else(|| crate::tr!("error-home-directory"))
}

fn preferred_home_directory(
    home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
    windows_native: bool,
) -> Option<PathBuf> {
    let selected = if windows_native {
        user_profile.or(home)
    } else {
        home.or(user_profile)
    };
    selected.map(PathBuf::from)
}

fn apply_runtime_port_detection(config: &mut AppConfig) {
    if let Some(port) = runtime_host_port(config, WINBOAT_API_GUEST_PORT, "tcp") {
        config.api_url = format!("http://127.0.0.1:{port}");
    }
    if let Some(port) = runtime_host_port(config, WINBOAT_RDP_GUEST_PORT, "tcp") {
        config.rdp_port = port;
    }
}

fn parse_runtime_port_output(output: &str) -> Option<u16> {
    output.lines().find_map(|line| {
        line.trim()
            .rsplit_once(':')
            .and_then(|(_, port)| port.trim().parse::<u16>().ok())
    })
}

fn inspect_host_port(output: &str, private_port: &str) -> Option<u16> {
    let ports = serde_json::from_str::<serde_json::Value>(output.trim()).ok()?;
    ports
        .get(private_port)?
        .as_array()?
        .iter()
        .find_map(|binding| binding.get("HostPort")?.as_str()?.parse::<u16>().ok())
}

pub(super) fn home_string() -> String {
    home_directory()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::runtime_host_port_for;
    use super::{
        inspect_host_port, migrated_windows_workspace, parse_runtime_port_output,
        preferred_home_directory,
    };
    #[cfg(unix)]
    use crate::process::{CommandFailureKind, CommandPolicy};
    use std::ffi::OsString;
    use std::path::PathBuf;
    #[cfg(unix)]
    use std::time::Duration;

    #[test]
    fn selects_the_platform_native_home_variable_first() {
        let home = Some(OsString::from("/home/dev"));
        let user_profile = Some(OsString::from(r"C:\Users\dev"));
        assert_eq!(
            preferred_home_directory(home.clone(), user_profile.clone(), true),
            Some(PathBuf::from(r"C:\Users\dev"))
        );
        assert_eq!(
            preferred_home_directory(home, user_profile, false),
            Some(PathBuf::from("/home/dev"))
        );
    }

    #[test]
    fn migrates_only_the_legacy_whole_home_windows_workspace() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let home = temporary.path();
        assert_eq!(
            migrated_windows_workspace(home, home, true),
            Some(home.join("Mendix"))
        );
        assert_eq!(
            migrated_windows_workspace(&home.join("Projects"), home, true),
            None
        );
        assert_eq!(migrated_windows_workspace(home, home, false), None);
    }

    #[test]
    fn parses_runtime_ports_from_docker_and_podman_output() {
        assert_eq!(parse_runtime_port_output("127.0.0.1:47283\n"), Some(47283));
        assert_eq!(parse_runtime_port_output("[::1]:47304\n"), Some(47304));
        assert_eq!(
            inspect_host_port(
                r#"{"7148/tcp":[{"HostIp":"127.0.0.1","HostPort":"47284"}]}"#,
                "7148/tcp"
            ),
            Some(47284)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_runtime_port_lookup_skips_inspect_fallback() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let log = temporary.path().join("calls.log");
        let runtime = executable_fixture(
            temporary.path(),
            "printf '%s\\n' \"$1\" >> \"$CALL_LOG\"\nif [ \"$1\" = port ]; then printf '127.0.0.1:47283\\n'; exit 0; fi\nprintf '{\"7148/tcp\":[{\"HostPort\":\"49999\"}]}'\n",
        );

        let port = runtime_host_port_for(
            runtime.to_str().expect("fixture path"),
            "WinBoat",
            7148,
            "tcp",
            CommandPolicy::PROBE,
        )
        .await
        .expect("runtime lookup succeeds");

        assert_eq!(port, Some(47283));
        assert_eq!(std::fs::read_to_string(log).expect("call log"), "port\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_runtime_port_lookup_uses_bounded_inspect_fallback() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let log = temporary.path().join("calls.log");
        let runtime = executable_fixture(
            temporary.path(),
            "printf '%s\\n' \"$1\" >> \"$CALL_LOG\"\nif [ \"$1\" = port ]; then exit 1; fi\nprintf '{\"7148/tcp\":[{\"HostPort\":\"47284\"}]}'\n",
        );

        let port = runtime_host_port_for(
            runtime.to_str().expect("fixture path"),
            "WinBoat",
            7148,
            "tcp",
            CommandPolicy::PROBE,
        )
        .await
        .expect("inspect fallback succeeds");

        assert_eq!(port, Some(47284));
        assert_eq!(
            std::fs::read_to_string(log).expect("call log"),
            "port\ninspect\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_terminating_fake_runtime_reaches_its_deadline() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let runtime = executable_fixture(
            temporary.path(),
            "trap '' TERM\nwhile :; do sleep 1; done\n",
        );

        let failure = runtime_host_port_for(
            runtime.to_str().expect("fixture path"),
            "WinBoat",
            7148,
            "tcp",
            CommandPolicy::new(Duration::from_millis(100), 1024),
        )
        .await
        .expect_err("runtime lookup times out");

        assert_eq!(failure.kind(), CommandFailureKind::Timeout);
    }

    #[cfg(unix)]
    fn executable_fixture(directory: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let executable = directory.join("fake-runtime");
        let call_log = directory
            .join("calls.log")
            .to_string_lossy()
            .replace('\'', "'\\''");
        let script = format!("#!/bin/sh\nCALL_LOG='{call_log}'\n{body}");
        std::fs::write(&executable, script).expect("write runtime fixture");
        let mut permissions = std::fs::metadata(&executable)
            .expect("runtime fixture metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).expect("make fixture executable");
        executable
    }
}
