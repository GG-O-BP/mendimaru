use crate::models::AppConfig;
use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct FileSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
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
        host_port_for_guest, restore_file, shared_mount_source, snapshot_file, update_shared_mount,
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
}
