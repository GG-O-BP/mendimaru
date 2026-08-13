use crate::models::{AppConfig, SettingsSaveResult};
use serde_yaml::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager};

const CONFIG_FILE_NAME: &str = "config.json";
const DEFAULT_API_PORT: u16 = 47280;
const DEFAULT_RDP_PORT: u16 = 47300;
const WINBOAT_API_GUEST_PORT: u16 = 7148;
const WINBOAT_RDP_GUEST_PORT: u16 = 3389;

pub fn load_config(app: &AppHandle) -> Result<AppConfig, String> {
    let path = config_path(app)?;
    if path.is_file() {
        let content = fs::read_to_string(&path)
            .map_err(|error| crate::tr!("error-config-read", error = error))?;
        if let Ok(mut config) = serde_json::from_str::<AppConfig>(&content) {
            refresh_detected_paths(&mut config);
            apply_runtime_port_detection(&mut config);
            return Ok(config);
        }
    }

    detect_config()
}

pub fn detect_config() -> Result<AppConfig, String> {
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
        container_runtime: runtime_for_compose(&compose_file).to_string(),
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
        startup_timeout_seconds: 180,
    };

    if compose_file.is_file() {
        if let Ok(compose) = read_compose(&compose_file) {
            apply_compose_detection(&mut config, &compose);
        }
    }
    apply_runtime_port_detection(&mut config);

    Ok(config)
}

pub async fn save_settings(
    app: &AppHandle,
    mut config: AppConfig,
    apply_mount: bool,
) -> Result<SettingsSaveResult, String> {
    normalize_and_validate(&mut config)?;

    let compose_path = PathBuf::from(&config.compose_file);
    let (mount_changed, backup_path) = if compose_path.is_file() {
        update_shared_mount(&compose_path, &config.shared_directory)?
    } else {
        (false, None)
    };
    persist_config(app, &config)?;

    let container_recreated = if mount_changed && apply_mount {
        crate::winboat::recreate_container(&config).await?;
        true
    } else {
        false
    };

    Ok(SettingsSaveResult {
        config,
        mount_changed,
        container_recreated,
        backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
    })
}

pub fn persist_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| crate::tr!("error-config-directory-create", error = error))?;
    }
    let serialized = serde_json::to_string_pretty(config)
        .map_err(|error| crate::tr!("error-config-serialize", error = error))?;
    fs::write(&path, serialized).map_err(|error| crate::tr!("error-config-save", error = error))
}

pub fn compose_shared_directory(compose_file: &str) -> Option<String> {
    let compose = read_compose(Path::new(compose_file)).ok()?;
    let volumes = service_value(&compose)?.get("volumes")?.as_sequence()?;
    volumes.iter().find_map(|volume| {
        volume
            .as_str()
            .and_then(shared_mount_source)
            .map(expand_compose_source)
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
    let output = Command::new(&config.container_runtime)
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

    let inspect = Command::new(&config.container_runtime)
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

fn refresh_detected_paths(config: &mut AppConfig) {
    let Ok(home) = home_directory() else {
        return;
    };
    if !Path::new(&expand_home(&config.winboat_executable)).is_file() {
        if let Some(executable) = discover_winboat_executable_with_home(&home) {
            config.winboat_executable = executable;
        }
    }

    if Path::new(&config.compose_file).is_file() {
        return;
    }
    let data_home = data_home_directory(&home);
    let Some(compose_file) = compose_candidates(&home, &data_home)
        .into_iter()
        .find(|candidate| candidate.is_file())
    else {
        return;
    };
    config.compose_file = compose_file.to_string_lossy().to_string();
    config.container_runtime = runtime_for_compose(&compose_file).to_string();
    if let Ok(compose) = read_compose(&compose_file) {
        let preferred_shared_directory = config.shared_directory.clone();
        apply_compose_detection(config, &compose);
        config.shared_directory = preferred_shared_directory;
    }
}

fn runtime_for_compose(compose_file: &Path) -> &'static str {
    if compose_file
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("podman"))
    {
        "podman"
    } else {
        "docker"
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

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(CONFIG_FILE_NAME))
        .map_err(|error| crate::tr!("error-app-config-path", error = error))
}

fn home_directory() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| crate::tr!("error-home-directory"))
}

fn normalize_and_validate(config: &mut AppConfig) -> Result<(), String> {
    config.language_preference = crate::i18n::normalize_preference(&config.language_preference)?;
    config.winboat_executable = expand_home(config.winboat_executable.trim());
    config.compose_file = expand_home(config.compose_file.trim());
    config.shared_directory = expand_home(config.shared_directory.trim());
    config.freerdp_binary = expand_home(config.freerdp_binary.trim());
    config.api_url = config.api_url.trim_end_matches('/').trim().to_string();
    config.rdp_host = config.rdp_host.trim().to_string();
    config.windows_shared_directory = config
        .windows_shared_directory
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.mendix_install_root = config
        .mendix_install_root
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.mendix_data_root = config
        .mendix_data_root
        .trim_end_matches(['\\', '/'])
        .trim()
        .to_string();
    config.container_runtime = config.container_runtime.trim().to_lowercase();
    config.container_name = config.container_name.trim().to_string();

    let shared = Path::new(&config.shared_directory);
    if !shared.is_absolute() || !shared.is_dir() {
        return Err(crate::tr!("error-shared-directory-invalid"));
    }
    config.shared_directory = shared
        .canonicalize()
        .map_err(|error| crate::tr!("error-shared-directory-inspect", error = error))?
        .to_string_lossy()
        .to_string();

    if !matches!(config.container_runtime.as_str(), "docker" | "podman") {
        return Err(crate::tr!("error-container-runtime-invalid"));
    }
    if config.compose_file.is_empty()
        || config.container_name.is_empty()
        || config.api_url.is_empty()
        || config.rdp_host.is_empty()
        || config.windows_shared_directory.is_empty()
    {
        return Err(crate::tr!("error-winboat-connection-empty"));
    }
    if config.startup_timeout_seconds == 0 || config.startup_timeout_seconds > 900 {
        return Err(crate::tr!(
            "error-startup-timeout-range",
            minimum = crate::i18n::format_number(1),
            maximum = crate::i18n::format_number(900)
        ));
    }

    Ok(())
}

fn read_compose(path: &Path) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| crate::tr!("error-compose-read", error = error))?;
    serde_yaml::from_str(&content).map_err(|error| crate::tr!("error-compose-parse", error = error))
}

fn write_compose(path: &Path, compose: &Value) -> Result<(), String> {
    let serialized = serde_yaml::to_string(compose)
        .map_err(|error| crate::tr!("error-compose-serialize", error = error))?;
    fs::write(path, serialized).map_err(|error| crate::tr!("error-compose-save", error = error))
}

fn apply_compose_detection(config: &mut AppConfig, compose: &Value) {
    let Some(service) = service_value(compose) else {
        return;
    };

    if let Some(name) = service.get("container_name").and_then(Value::as_str) {
        config.container_name = name.to_string();
    }

    if let Some(volumes) = service.get("volumes").and_then(Value::as_sequence) {
        if let Some(shared) = volumes.iter().find_map(|volume| {
            volume
                .as_str()
                .and_then(shared_mount_source)
                .map(ToString::to_string)
        }) {
            config.shared_directory = expand_compose_source(&shared);
        }
    }

    if let Some(ports) = service.get("ports").and_then(Value::as_sequence) {
        if let Some(port) = host_port_for_guest(ports, 7148) {
            config.api_url = format!("http://127.0.0.1:{port}");
        }
        if let Some(port) = host_port_for_guest(ports, 3389) {
            config.rdp_port = port;
        }
    }
}

fn apply_runtime_port_detection(config: &mut AppConfig) {
    if let Some(port) = runtime_host_port(config, WINBOAT_API_GUEST_PORT, "tcp") {
        config.api_url = format!("http://127.0.0.1:{port}");
    }
    if let Some(port) = runtime_host_port(config, WINBOAT_RDP_GUEST_PORT, "tcp") {
        config.rdp_port = port;
    }
}

fn home_string() -> String {
    home_directory()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

fn host_port_for_guest(ports: &[Value], guest_port: u16) -> Option<u16> {
    ports.iter().find_map(|port| {
        let raw = port.as_str()?;
        let parts: Vec<&str> = raw.split(':').collect();
        if parts.len() < 2 {
            return None;
        }
        let guest = parts.last()?.split('/').next()?.parse::<u16>().ok()?;
        if guest != guest_port {
            return None;
        }
        let host = parts.get(parts.len().saturating_sub(2))?;
        host.split('-').next()?.parse::<u16>().ok()
    })
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

fn expand_compose_source(source: &str) -> String {
    let home = home_string();
    expand_home(&source.replace("${HOME}", &home).replace("$HOME", &home))
}

fn shared_mount_source(volume: &str) -> Option<&str> {
    let marker = ":/shared";
    let marker_index = volume.rfind(marker)?;
    let suffix = &volume[marker_index + marker.len()..];
    if !suffix.is_empty() && !suffix.starts_with(':') {
        return None;
    }
    Some(&volume[..marker_index])
}

fn update_shared_mount(
    compose_path: &Path,
    shared_directory: &str,
) -> Result<(bool, Option<PathBuf>), String> {
    let mut compose = read_compose(compose_path)?;
    let current = service_value(&compose)
        .and_then(|service| service.get("volumes"))
        .and_then(Value::as_sequence)
        .and_then(|volumes| {
            volumes.iter().find_map(|volume| {
                volume
                    .as_str()
                    .and_then(shared_mount_source)
                    .map(ToString::to_string)
            })
        });

    if current.as_deref().map(expand_compose_source).as_deref() == Some(shared_directory) {
        return Ok((false, None));
    }

    let backup_path = compose_path.with_extension("yml.mendimaru.bak");
    if !backup_path.exists() {
        fs::copy(compose_path, &backup_path)
            .map_err(|error| crate::tr!("error-compose-backup", error = error))?;
    }

    let service = service_value_mut(&mut compose)
        .ok_or_else(|| crate::tr!("error-compose-windows-service-missing"))?;
    let mapping = service
        .as_mapping_mut()
        .ok_or_else(|| crate::tr!("error-compose-windows-service-invalid"))?;
    let volumes_key = Value::String("volumes".to_string());
    if !mapping.contains_key(&volumes_key) {
        mapping.insert(volumes_key.clone(), Value::Sequence(Vec::new()));
    }
    let volumes = mapping
        .get_mut(&volumes_key)
        .and_then(Value::as_sequence_mut)
        .ok_or_else(|| crate::tr!("error-compose-volumes-invalid"))?;

    let replacement = Value::String(format!("{shared_directory}:/shared"));
    if let Some(existing) = volumes
        .iter_mut()
        .find(|volume| volume.as_str().and_then(shared_mount_source).is_some())
    {
        *existing = replacement;
    } else {
        volumes.push(replacement);
    }

    write_compose(compose_path, &compose)?;
    Ok((true, Some(backup_path)))
}

fn service_value(compose: &Value) -> Option<&Value> {
    let services = compose.get("services")?.as_mapping()?;
    services
        .get(Value::String("windows".to_string()))
        .or_else(|| services.values().next())
}

fn service_value_mut(compose: &mut Value) -> Option<&mut Value> {
    let services = compose.get_mut("services")?.as_mapping_mut()?;
    let windows_key = Value::String("windows".to_string());
    if services.contains_key(&windows_key) {
        services.get_mut(&windows_key)
    } else {
        let first_key = services.keys().next()?.clone();
        services.get_mut(&first_key)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        home_string, host_port_for_guest, inspect_host_port, parse_runtime_port_output,
        shared_mount_source, update_shared_mount,
    };
    use serde_yaml::Value;
    use std::fs;

    #[test]
    fn extracts_shared_mount_source_with_options() {
        assert_eq!(
            shared_mount_source("/home/dev/Mendix:/shared"),
            Some("/home/dev/Mendix")
        );
        assert_eq!(
            shared_mount_source("/home/dev/Mendix:/shared:rw"),
            Some("/home/dev/Mendix")
        );
        assert_eq!(shared_mount_source("data:/storage"), None);
    }

    #[test]
    fn extracts_host_ports_from_compose_values() {
        let ports: Vec<Value> =
            serde_yaml::from_str("- 127.0.0.1:47271:7148\n- 127.0.0.1:47273:3389/tcp\n")
                .expect("ports yaml");
        assert_eq!(host_port_for_guest(&ports, 7148), Some(47271));
        assert_eq!(host_port_for_guest(&ports, 3389), Some(47273));
    }

    #[test]
    fn extracts_winboat_09_port_range_fallbacks() {
        let ports: Vec<Value> = serde_yaml::from_str(
            "- 127.0.0.1:47280-47289:7148\n- 127.0.0.1:47300-47309:3389/tcp\n- 127.0.0.1::8006\n",
        )
        .expect("ports yaml");
        assert_eq!(host_port_for_guest(&ports, 7148), Some(47280));
        assert_eq!(host_port_for_guest(&ports, 3389), Some(47300));
        assert_eq!(host_port_for_guest(&ports, 8006), None);
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

    #[test]
    fn updates_only_shared_volume_and_creates_backup() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: test\n    volumes:\n      - /old:/shared\n      - data:/storage\n",
        )
        .expect("write compose");

        let (changed, backup) =
            update_shared_mount(&compose, "/new/workspace").expect("update mount");
        assert!(changed);
        assert!(backup.expect("backup path").is_file());
        let updated = fs::read_to_string(compose).expect("read compose");
        assert!(updated.contains("/new/workspace:/shared"));
        assert!(updated.contains("data:/storage"));
    }

    #[test]
    fn treats_home_variable_mount_as_the_same_directory() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: test\n    volumes:\n      - ${HOME}:/shared\n",
        )
        .expect("write compose");

        let (changed, backup) =
            update_shared_mount(&compose, &home_string()).expect("compare mount");
        assert!(!changed);
        assert!(backup.is_none());
    }
}
