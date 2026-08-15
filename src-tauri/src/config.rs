mod compose;
mod store;
mod validation;

use crate::models::{AppConfig, ContainerRuntime};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use compose::{compose_file_is_valid, compose_shared_directory};
pub(crate) use compose::{ensure_runtime_port_mapping, runtime_port_mapping};
pub(crate) use compose::{restore_file, snapshot_file, update_shared_mount, FileSnapshot};
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
            .unwrap_or_else(|| "xfreerdp3".to_string()),
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
    let home = home_directory()?;
    let default_workspace = home.join("Mendix");
    let shared_directory = if default_workspace.is_dir() {
        default_workspace
    } else {
        home
    };
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
        shared_directory: shared_directory.to_string_lossy().to_string(),
        windows_shared_directory: String::new(),
        freerdp_binary: String::new(),
        mendix_install_root: program_files.join("Mendix").to_string_lossy().to_string(),
        mendix_data_root: program_data.join("Mendix").to_string_lossy().to_string(),
        windows_studio_paths: Vec::new(),
        startup_timeout_seconds: 180,
    })
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

pub fn resolved_api_url(config: &AppConfig) -> String {
    runtime_host_port(config, WINBOAT_API_GUEST_PORT, "tcp")
        .map(|port| format!("http://127.0.0.1:{port}"))
        .unwrap_or_else(|| config.api_url.clone())
}

pub fn resolved_rdp_port(config: &AppConfig) -> u16 {
    runtime_host_port(config, WINBOAT_RDP_GUEST_PORT, "tcp").unwrap_or(config.rdp_port)
}

pub fn runtime_host_port(config: &AppConfig, guest_port: u16, protocol: &str) -> Option<u16> {
    let private_port = format!("{guest_port}/{protocol}");
    let output = Command::new(config.container_runtime.as_str())
        .arg("port")
        .arg(&config.container_name)
        .arg(&private_port)
        .output()
        .ok()?;
    if output.status.success() {
        if let Some(port) = parse_runtime_port_output(&String::from_utf8_lossy(&output.stdout)) {
            return Some(port);
        }
    }

    let inspect = Command::new(config.container_runtime.as_str())
        .arg("inspect")
        .arg("--format")
        .arg("{{json .NetworkSettings.Ports}}")
        .arg(&config.container_name)
        .output()
        .ok()?;
    if !inspect.status.success() {
        return None;
    }
    inspect_host_port(&String::from_utf8_lossy(&inspect.stdout), &private_port)
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
    use super::{inspect_host_port, parse_runtime_port_output, preferred_home_directory};
    use std::ffi::OsString;
    use std::path::PathBuf;

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
}
