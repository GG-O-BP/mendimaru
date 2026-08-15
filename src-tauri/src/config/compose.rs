use crate::models::AppConfig;
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct FileSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

const LOOPBACK_IPV4: &str = "127.0.0.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimePortMapping {
    pub(crate) host_ip: String,
    pub(crate) host_port: Option<u16>,
    pub(crate) guest_port: u16,
    pub(crate) protocol: String,
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

pub fn compose_file_is_valid(compose_file: &str) -> bool {
    read_compose(Path::new(compose_file))
        .ok()
        .and_then(|compose| service_value(&compose).cloned())
        .is_some_and(|service| service.is_mapping())
}

pub(super) fn read_compose(path: &Path) -> Result<Value, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| crate::tr!("error-compose-read", error = error))?;
    serde_yaml::from_str(&content).map_err(|error| crate::tr!("error-compose-parse", error = error))
}

fn write_compose(path: &Path, compose: &Value) -> Result<(), String> {
    let serialized = serde_yaml::to_string(compose)
        .map_err(|error| crate::tr!("error-compose-serialize", error = error))?;
    let temporary_path = path.with_extension("yml.mendimaru.tmp");
    fs::write(&temporary_path, serialized)
        .map_err(|error| crate::tr!("error-compose-save", error = error))?;
    fs::rename(&temporary_path, path)
        .map_err(|error| crate::tr!("error-compose-save", error = error))
}

pub(super) fn apply_compose_detection(config: &mut AppConfig, compose: &Value) {
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

fn host_port_for_guest(ports: &[Value], guest_port: u16) -> Option<u16> {
    ports.iter().find_map(|port| {
        let mapping = parse_port_mapping(port)?;
        (mapping.guest_port == guest_port).then_some(mapping.host_port)?
    })
}

pub(crate) fn runtime_port_mapping(
    compose_path: &Path,
    guest_port: u16,
) -> Result<Option<RuntimePortMapping>, String> {
    let compose = read_compose(compose_path)?;
    let ports = service_value(&compose)
        .and_then(|service| service.get("ports"))
        .and_then(Value::as_sequence);
    let Some(ports) = ports else {
        return Ok(None);
    };
    let mappings = ports
        .iter()
        .filter_map(parse_port_mapping)
        .filter(|mapping| mapping.guest_port == guest_port)
        .collect::<Vec<_>>();
    match mappings.as_slice() {
        [] => Ok(None),
        [mapping] => Ok(Some(mapping.clone())),
        _ => Err(format!(
            "the Compose service contains multiple mappings for guest port {guest_port}"
        )),
    }
}

pub(crate) fn ensure_runtime_port_mapping(
    compose_path: &Path,
    guest_port: u16,
) -> Result<bool, String> {
    if !(1024..=u16::MAX).contains(&guest_port) {
        return Err("the Mendix Runtime guest port must be from 1024 through 65535".to_string());
    }
    let mut compose = read_compose(compose_path)?;
    let service = service_value_mut(&mut compose)
        .ok_or_else(|| crate::tr!("error-compose-windows-service-missing"))?;
    let mapping = service
        .as_mapping_mut()
        .ok_or_else(|| crate::tr!("error-compose-windows-service-invalid"))?;
    if mapping
        .get(Value::String("network_mode".to_string()))
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case("host"))
    {
        return Err("the WinBoat service uses host networking and cannot be isolated".to_string());
    }

    let volumes = mapping
        .get(Value::String("volumes".to_string()))
        .and_then(Value::as_sequence)
        .ok_or_else(|| "the WinBoat service has no protected /storage volume".to_string())?;
    let storage_before = storage_mounts(volumes);
    if storage_before.is_empty() {
        return Err("the WinBoat service has no protected /storage volume".to_string());
    }

    let ports_key = Value::String("ports".to_string());
    if !mapping.contains_key(&ports_key) {
        mapping.insert(ports_key.clone(), Value::Sequence(Vec::new()));
    }
    let ports = mapping
        .get_mut(&ports_key)
        .and_then(Value::as_sequence_mut)
        .ok_or_else(|| "the WinBoat Compose ports value is invalid".to_string())?;
    let expected = RuntimePortMapping {
        host_ip: LOOPBACK_IPV4.to_string(),
        host_port: None,
        guest_port,
        protocol: "tcp".to_string(),
    };
    let current = ports
        .iter()
        .filter_map(parse_port_mapping)
        .filter(|entry| entry.guest_port == guest_port)
        .collect::<Vec<_>>();
    if current.as_slice() == [expected.clone()] {
        return Ok(false);
    }
    ports.retain(|entry| {
        parse_port_mapping(entry).is_none_or(|mapping| mapping.guest_port != guest_port)
    });
    ports.push(Value::String(format!("{LOOPBACK_IPV4}::{guest_port}/tcp")));

    let storage_after = mapping
        .get(Value::String("volumes".to_string()))
        .and_then(Value::as_sequence)
        .map(|volumes| storage_mounts(volumes))
        .unwrap_or_default();
    if storage_after != storage_before {
        return Err(
            "the WinBoat /storage volume changed while adding Runtime forwarding".to_string(),
        );
    }
    write_compose(compose_path, &compose)?;
    Ok(true)
}

fn parse_port_mapping(value: &Value) -> Option<RuntimePortMapping> {
    if let Some(guest_port) = yaml_u16(value) {
        return Some(RuntimePortMapping {
            host_ip: String::new(),
            host_port: None,
            guest_port,
            protocol: "tcp".to_string(),
        });
    }
    if let Some(raw) = value.as_str() {
        return parse_short_port_mapping(raw);
    }
    let mapping = value.as_mapping()?;
    let guest_port = yaml_u16(mapping.get(Value::String("target".to_string()))?)?;
    let host_port = mapping
        .get(Value::String("published".to_string()))
        .and_then(yaml_u16);
    let host_ip = mapping
        .get(Value::String("host_ip".to_string()))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let protocol = mapping
        .get(Value::String("protocol".to_string()))
        .and_then(Value::as_str)
        .unwrap_or("tcp")
        .to_ascii_lowercase();
    Some(RuntimePortMapping {
        host_ip,
        host_port,
        guest_port,
        protocol,
    })
}

fn parse_short_port_mapping(raw: &str) -> Option<RuntimePortMapping> {
    let (raw, protocol) = raw
        .rsplit_once('/')
        .map(|(mapping, protocol)| (mapping, protocol.to_ascii_lowercase()))
        .unwrap_or((raw, "tcp".to_string()));
    let parts = raw.split(':').collect::<Vec<_>>();
    let guest_port = parts.last()?.parse::<u16>().ok()?;
    let (host_ip, host_port) = match parts.as_slice() {
        [_guest] => (String::new(), None),
        [published, _guest] => (String::new(), published.parse::<u16>().ok()),
        [host_ip @ .., published, _guest] => (
            host_ip.join(":"),
            (!published.is_empty())
                .then(|| published.split('-').next()?.parse::<u16>().ok())
                .flatten(),
        ),
        _ => return None,
    };
    Some(RuntimePortMapping {
        host_ip,
        host_port,
        guest_port,
        protocol,
    })
}

fn yaml_u16(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| value.as_str()?.parse::<u16>().ok())
}

fn storage_mounts(volumes: &[Value]) -> Vec<Value> {
    volumes
        .iter()
        .filter(|volume| {
            volume
                .as_str()
                .is_some_and(|raw| volume_target(raw) == Some("/storage"))
                || volume
                    .as_mapping()
                    .and_then(|mapping| mapping.get(Value::String("target".to_string())))
                    .and_then(Value::as_str)
                    == Some("/storage")
        })
        .cloned()
        .collect()
}

fn volume_target(volume: &str) -> Option<&str> {
    let mut parts = volume.rsplit(':');
    let last = parts.next()?;
    if last.starts_with('/') {
        Some(last)
    } else {
        parts.next().filter(|target| target.starts_with('/'))
    }
}

fn expand_compose_source(source: &str) -> String {
    let home = super::home_string();
    super::expand_home(&source.replace("${HOME}", &home).replace("$HOME", &home))
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

pub(crate) fn update_shared_mount(
    compose_path: &Path,
    shared_directory: &str,
) -> Result<bool, String> {
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
        return Ok(false);
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
    Ok(true)
}

pub(crate) fn snapshot_file(path: &Path) -> Result<FileSnapshot, String> {
    let content = if path.is_file() {
        Some(fs::read(path).map_err(|error| crate::tr!("error-compose-read", error = error))?)
    } else {
        None
    };
    Ok(FileSnapshot {
        path: path.to_path_buf(),
        content,
    })
}

pub(crate) fn restore_file(snapshot: &FileSnapshot) -> Result<(), String> {
    match &snapshot.content {
        Some(content) => {
            let temporary_path = snapshot.path.with_extension("yml.mendimaru.restore.tmp");
            fs::write(&temporary_path, content)
                .map_err(|error| crate::tr!("error-compose-save", error = error))?;
            fs::rename(&temporary_path, &snapshot.path)
                .map_err(|error| crate::tr!("error-compose-save", error = error))
        }
        None if snapshot.path.exists() => fs::remove_file(&snapshot.path)
            .map_err(|error| crate::tr!("error-compose-save", error = error)),
        None => Ok(()),
    }
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
        compose_file_is_valid, ensure_runtime_port_mapping, host_port_for_guest, restore_file,
        runtime_port_mapping, shared_mount_source, snapshot_file, update_shared_mount,
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
    fn extracts_winboat_port_range_fallbacks() {
        let ports: Vec<Value> = serde_yaml::from_str(
            "- 127.0.0.1:47280-47289:7148\n- 127.0.0.1:47300-47309:3389/tcp\n- 127.0.0.1::8006\n",
        )
        .expect("ports yaml");
        assert_eq!(host_port_for_guest(&ports, 7148), Some(47280));
        assert_eq!(host_port_for_guest(&ports, 3389), Some(47300));
        assert_eq!(host_port_for_guest(&ports, 8006), None);
    }

    #[test]
    fn adds_only_a_dynamic_loopback_runtime_mapping_and_preserves_storage() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: test\n    volumes:\n      - winboat-data:/storage\n      - /workspace:/shared\n    ports:\n      - 127.0.0.1:47280:7148\n      - 0.0.0.0:8080:8080\nvolumes:\n  winboat-data: {}\n",
        )
        .expect("write compose");

        assert!(ensure_runtime_port_mapping(&compose, 8080).expect("add mapping"));
        let updated = fs::read_to_string(&compose).expect("read compose");
        assert!(updated.contains("winboat-data:/storage"));
        assert!(updated.contains("127.0.0.1::8080/tcp"));
        assert!(!updated.contains("0.0.0.0:8080:8080"));
        assert!(!ensure_runtime_port_mapping(&compose, 8080).expect("mapping is stable"));
        let mapping = runtime_port_mapping(&compose, 8080)
            .expect("inspect mapping")
            .expect("mapping exists");
        assert_eq!(mapping.host_ip, "127.0.0.1");
        assert_eq!(mapping.host_port, None);
        assert_eq!(mapping.guest_port, 8080);
    }

    #[test]
    fn refuses_host_networking_or_a_compose_without_protected_storage() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: test\n    network_mode: host\n    volumes:\n      - data:/storage\n",
        )
        .expect("write compose");
        assert!(ensure_runtime_port_mapping(&compose, 8080)
            .expect_err("host networking rejected")
            .contains("host networking"));

        fs::write(
            &compose,
            "services:\n  windows:\n    image: test\n    volumes:\n      - /workspace:/shared\n",
        )
        .expect("write compose");
        assert!(ensure_runtime_port_mapping(&compose, 8080)
            .expect_err("missing storage rejected")
            .contains("/storage"));
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
        assert!(update_shared_mount(&compose, "/new/workspace").expect("update mount"));
        assert!(compose.with_extension("yml.mendimaru.bak").is_file());
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
        assert!(
            !update_shared_mount(&compose, &super::super::home_string()).expect("compare mount")
        );
    }

    #[test]
    fn restores_a_compose_snapshot_after_a_failed_transaction() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        let original = "services:\n  windows:\n    volumes:\n      - /old:/shared\n";
        fs::write(&compose, original).expect("write original compose");
        let snapshot = snapshot_file(&compose).expect("snapshot compose");

        fs::write(&compose, "changed").expect("change compose");
        restore_file(&snapshot).expect("restore compose");

        assert_eq!(fs::read_to_string(compose).expect("read compose"), original);
    }

    #[test]
    fn distinguishes_valid_compose_from_yaml_without_a_service() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(&compose, "services:\n  windows:\n    image: test\n")
            .expect("write valid compose");
        assert!(compose_file_is_valid(&compose.to_string_lossy()));

        fs::write(&compose, "name: parsed-but-not-compose\n").expect("write invalid compose");
        assert!(!compose_file_is_valid(&compose.to_string_lossy()));

        fs::write(&compose, "services: [unterminated\n").expect("write malformed compose");
        assert!(!compose_file_is_valid(&compose.to_string_lossy()));
    }
}
