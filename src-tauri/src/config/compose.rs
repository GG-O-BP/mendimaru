use crate::models::AppConfig;
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct FileSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposeErrorKind {
    NotWinboat,
    Ambiguous,
    RevisionConflict,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposeError {
    kind: ComposeErrorKind,
    message: String,
}

impl ComposeError {
    pub(crate) fn kind(&self) -> ComposeErrorKind {
        self.kind
    }

    fn new(kind: ComposeErrorKind, message: String) -> Self {
        Self { kind, message }
    }
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<String> for ComposeError {
    fn from(message: String) -> Self {
        Self {
            kind: ComposeErrorKind::Other,
            message,
        }
    }
}

impl From<ComposeError> for String {
    fn from(error: ComposeError) -> Self {
        error.message
    }
}

#[derive(Debug)]
pub(crate) struct SharedMountPlan {
    compose: Value,
    snapshot: FileSnapshot,
    service_name: String,
    current_source: Option<String>,
    next_source: String,
    revision: String,
    mount_changed: bool,
}

impl SharedMountPlan {
    pub(crate) fn service_name(&self) -> &str {
        &self.service_name
    }

    pub(crate) fn current_source(&self) -> Option<&str> {
        self.current_source.as_deref()
    }

    pub(crate) fn next_source(&self) -> &str {
        &self.next_source
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) const fn mount_changed(&self) -> bool {
        self.mount_changed
    }

    pub(crate) fn apply_detection(&self, config: &mut AppConfig) {
        let requested_shared_directory = config.shared_directory.clone();
        if let Some(service) = service_value_named(&self.compose, &self.service_name) {
            apply_service_detection(config, service);
        }
        config.shared_directory = requested_shared_directory;
    }
}

impl FileSnapshot {
    fn revision(&self) -> Option<String> {
        self.content.as_deref().map(revision_for)
    }
}

#[derive(Debug)]
pub(crate) struct AppliedSharedMount {
    pub(crate) changed: bool,
    pub(crate) service_name: String,
    pub(crate) original: FileSnapshot,
    pub(crate) applied_revision: String,
}

const LOOPBACK_IPV4: &str = "127.0.0.1";
const MAX_COMPOSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimePortMapping {
    pub(crate) host_ip: String,
    pub(crate) host_port: Option<u16>,
    pub(crate) guest_port: u16,
    pub(crate) protocol: String,
}

pub fn compose_shared_directory(compose_file: &str) -> Option<String> {
    let compose = read_compose(Path::new(compose_file)).ok()?;
    let service_name = winboat_service_name(&compose).ok()?;
    let service = service_value_named(&compose, &service_name)?;
    shared_mount_source_from_service(service)
        .ok()?
        .map(|source| expand_compose_source(&source))
}

pub fn compose_file_is_valid(compose_file: &str) -> bool {
    read_compose(Path::new(compose_file))
        .ok()
        .is_some_and(|compose| winboat_service_name(&compose).is_ok())
}

pub(super) fn read_compose(path: &Path) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| crate::tr!("error-compose-read", error = error))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(crate::tr!(
            "error-compose-read",
            error = "the Compose file must be a regular file"
        ));
    }
    if metadata.len() > MAX_COMPOSE_BYTES {
        return Err(crate::tr!(
            "error-compose-read",
            error = "the Compose file exceeds the safe size limit"
        ));
    }
    let file =
        fs::File::open(path).map_err(|error| crate::tr!("error-compose-read", error = error))?;
    let mut content = String::with_capacity(metadata.len() as usize);
    file.take(MAX_COMPOSE_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|error| crate::tr!("error-compose-read", error = error))?;
    serde_yaml::from_str(&content).map_err(|error| crate::tr!("error-compose-parse", error = error))
}

fn write_compose(path: &Path, compose: &Value, expected_revision: &str) -> Result<(), String> {
    let serialized = serde_yaml::to_string(compose)
        .map_err(|error| crate::tr!("error-compose-serialize", error = error))?;
    let temporary_path = path.with_extension("yml.mendimaru.tmp");
    atomic_replace(
        path,
        &temporary_path,
        serialized.as_bytes(),
        Some(expected_revision),
    )
}

pub(super) fn apply_compose_detection(config: &mut AppConfig, compose: &Value) {
    let Ok(service_name) = winboat_service_name(compose) else {
        return;
    };
    let Some(service) = service_value_named(compose, &service_name) else {
        return;
    };
    apply_service_detection(config, service);
}

fn apply_service_detection(config: &mut AppConfig, service: &Value) {
    if let Some(name) = service.get("container_name").and_then(Value::as_str) {
        config.container_name = name.to_string();
    }
    if let Ok(Some(shared)) = shared_mount_source_from_service(service) {
        config.shared_directory = expand_compose_source(&shared);
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
    let service_name = winboat_service_name(&compose).map_err(String::from)?;
    let ports = service_value_named(&compose, &service_name)
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
    let snapshot = snapshot_file(compose_path)?;
    let revision = snapshot.revision().ok_or_else(|| {
        crate::tr!(
            "error-compose-file-not-found",
            path = compose_path.display()
        )
    })?;
    let mut compose = parse_snapshot(&snapshot)?;
    let service_name = winboat_service_name(&compose).map_err(String::from)?;
    let service = service_value_named_mut(&mut compose, &service_name)
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
        host_port: Some(guest_port),
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
    ports.push(Value::String(format!(
        "{LOOPBACK_IPV4}:{guest_port}:{guest_port}/tcp"
    )));

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
    write_compose(compose_path, &compose, &revision)?;
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
        .filter(|volume| volume_target_value(volume) == Some("/storage"))
        .cloned()
        .collect()
}

fn volume_target_value(volume: &Value) -> Option<&str> {
    volume.as_str().and_then(volume_target).or_else(|| {
        volume
            .as_mapping()
            .and_then(|mapping| mapping.get(Value::String("target".to_string())))
            .and_then(Value::as_str)
    })
}

fn volume_source_value(volume: &Value) -> Option<&str> {
    if let Some(raw) = volume.as_str() {
        return match volume_target(raw)? {
            "/shared" => shared_mount_source(raw),
            _ => raw.split_once(':').map(|(source, _)| source),
        };
    }
    volume
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String("source".to_string())))
        .and_then(Value::as_str)
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

fn shared_mount_source_from_service(service: &Value) -> Result<Option<String>, ComposeError> {
    let Some(volumes) = service.get("volumes") else {
        return Ok(None);
    };
    let volumes = volumes.as_sequence().ok_or_else(|| {
        ComposeError::new(
            ComposeErrorKind::Other,
            crate::tr!("error-compose-volumes-invalid"),
        )
    })?;
    let shared = volumes
        .iter()
        .filter(|volume| volume_target_value(volume) == Some("/shared"))
        .collect::<Vec<_>>();
    match shared.as_slice() {
        [] => Ok(None),
        [volume] => volume_source_value(volume)
            .map(|source| Some(source.to_string()))
            .ok_or_else(|| {
                ComposeError::new(
                    ComposeErrorKind::Other,
                    crate::tr!("error-compose-volumes-invalid"),
                )
            }),
        _ => Err(ComposeError::new(
            ComposeErrorKind::Other,
            crate::tr!("error-compose-shared-mount-ambiguous"),
        )),
    }
}

pub(crate) fn plan_shared_mount(
    compose_path: &Path,
    shared_directory: &str,
) -> Result<SharedMountPlan, ComposeError> {
    let snapshot = snapshot_file(compose_path).map_err(ComposeError::from)?;
    let revision = snapshot.revision().ok_or_else(|| {
        ComposeError::new(
            ComposeErrorKind::Other,
            crate::tr!(
                "error-compose-file-not-found",
                path = compose_path.display()
            ),
        )
    })?;
    let compose = parse_snapshot(&snapshot).map_err(ComposeError::from)?;
    let service_name = winboat_service_name(&compose)?;
    let service = service_value_named(&compose, &service_name).ok_or_else(|| {
        ComposeError::new(
            ComposeErrorKind::Other,
            crate::tr!("error-compose-windows-service-missing"),
        )
    })?;
    let current_source = shared_mount_source_from_service(service)?;
    let mount_changed = current_source
        .as_deref()
        .map(expand_compose_source)
        .as_deref()
        != Some(shared_directory);

    Ok(SharedMountPlan {
        compose,
        snapshot,
        service_name,
        current_source,
        next_source: shared_directory.to_string(),
        revision,
        mount_changed,
    })
}

pub(crate) fn verify_plan_revision(
    plan: &SharedMountPlan,
    expected_revision: &str,
) -> Result<(), ComposeError> {
    if plan.revision == expected_revision {
        Ok(())
    } else {
        Err(revision_conflict())
    }
}

pub(crate) fn apply_shared_mount(
    mut plan: SharedMountPlan,
) -> Result<AppliedSharedMount, ComposeError> {
    if !plan.mount_changed {
        verify_current_revision(&plan.snapshot.path, &plan.revision)?;
        return Ok(AppliedSharedMount {
            changed: false,
            service_name: plan.service_name,
            original: plan.snapshot,
            applied_revision: plan.revision,
        });
    }

    let backup_path = plan.snapshot.path.with_extension("yml.mendimaru.bak");
    match fs::symlink_metadata(&backup_path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(ComposeError::from(crate::tr!(
                "error-compose-backup",
                error = "the Compose backup file is unsafe"
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            verify_current_revision(&plan.snapshot.path, &plan.revision)?;
            create_backup(&plan.snapshot, &backup_path).map_err(ComposeError::from)?;
        }
        Err(error) => {
            return Err(ComposeError::from(crate::tr!(
                "error-compose-backup",
                error = error
            )))
        }
    }

    let service =
        service_value_named_mut(&mut plan.compose, &plan.service_name).ok_or_else(|| {
            ComposeError::new(
                ComposeErrorKind::Other,
                crate::tr!("error-compose-windows-service-missing"),
            )
        })?;
    let mapping = service
        .as_mapping_mut()
        .ok_or_else(|| ComposeError::from(crate::tr!("error-compose-windows-service-invalid")))?;
    let volumes_key = Value::String("volumes".to_string());
    if !mapping.contains_key(&volumes_key) {
        mapping.insert(volumes_key.clone(), Value::Sequence(Vec::new()));
    }
    let volumes = mapping
        .get_mut(&volumes_key)
        .and_then(Value::as_sequence_mut)
        .ok_or_else(|| ComposeError::from(crate::tr!("error-compose-volumes-invalid")))?;
    let storage_before = storage_mounts(volumes);
    if let Some(existing) = volumes
        .iter_mut()
        .find(|volume| volume_target_value(volume) == Some("/shared"))
    {
        replace_shared_mount_source(existing, &plan.next_source)?;
    } else {
        volumes.push(Value::String(format!("{}:/shared", plan.next_source)));
    }
    let storage_after = storage_mounts(volumes);
    if storage_after != storage_before {
        return Err(ComposeError::from(
            "the WinBoat /storage volume changed while updating /shared".to_string(),
        ));
    }

    let serialized = serde_yaml::to_string(&plan.compose).map_err(|error| {
        ComposeError::from(crate::tr!("error-compose-serialize", error = error))
    })?;
    let applied_revision = revision_for(serialized.as_bytes());
    let temporary_path = plan.snapshot.path.with_extension("yml.mendimaru.tmp");
    atomic_replace(
        &plan.snapshot.path,
        &temporary_path,
        serialized.as_bytes(),
        Some(&plan.revision),
    )
    .map_err(compose_write_error)?;

    Ok(AppliedSharedMount {
        changed: true,
        service_name: plan.service_name,
        original: plan.snapshot,
        applied_revision,
    })
}

fn replace_shared_mount_source(
    volume: &mut Value,
    shared_directory: &str,
) -> Result<(), ComposeError> {
    if let Some(raw) = volume.as_str() {
        let marker = ":/shared";
        let marker_index = raw
            .rfind(marker)
            .ok_or_else(|| ComposeError::from(crate::tr!("error-compose-volumes-invalid")))?;
        let suffix = &raw[marker_index + marker.len()..];
        *volume = Value::String(format!("{shared_directory}{marker}{suffix}"));
        return Ok(());
    }
    let mapping = volume
        .as_mapping_mut()
        .ok_or_else(|| ComposeError::from(crate::tr!("error-compose-volumes-invalid")))?;
    mapping.insert(
        Value::String("source".to_string()),
        Value::String(shared_directory.to_string()),
    );
    Ok(())
}

fn compose_write_error(message: String) -> ComposeError {
    if message == crate::tr!("error-compose-revision-conflict") {
        revision_conflict()
    } else {
        ComposeError::from(message)
    }
}

fn revision_conflict() -> ComposeError {
    ComposeError::new(
        ComposeErrorKind::RevisionConflict,
        crate::tr!("error-compose-revision-conflict"),
    )
}

#[cfg(test)]
fn update_shared_mount(compose_path: &Path, shared_directory: &str) -> Result<bool, String> {
    let plan = plan_shared_mount(compose_path, shared_directory).map_err(String::from)?;
    apply_shared_mount(plan)
        .map(|applied| applied.changed)
        .map_err(String::from)
}

pub(crate) fn snapshot_file(path: &Path) -> Result<FileSnapshot, String> {
    let content = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(crate::tr!(
                    "error-compose-read",
                    error = "the Compose file must be a regular file"
                ));
            }
            if metadata.len() > MAX_COMPOSE_BYTES {
                return Err(crate::tr!(
                    "error-compose-read",
                    error = "the Compose file exceeds the safe size limit"
                ));
            }
            let mut content = Vec::with_capacity(metadata.len() as usize);
            fs::File::open(path)
                .map_err(|error| crate::tr!("error-compose-read", error = error))?
                .take(MAX_COMPOSE_BYTES + 1)
                .read_to_end(&mut content)
                .map_err(|error| crate::tr!("error-compose-read", error = error))?;
            if content.len() as u64 > MAX_COMPOSE_BYTES {
                return Err(crate::tr!(
                    "error-compose-read",
                    error = "the Compose file exceeds the safe size limit"
                ));
            }
            Some(content)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(crate::tr!("error-compose-read", error = error)),
    };
    Ok(FileSnapshot {
        path: path.to_path_buf(),
        content,
    })
}

fn parse_snapshot(snapshot: &FileSnapshot) -> Result<Value, String> {
    let content = snapshot.content.as_deref().ok_or_else(|| {
        crate::tr!(
            "error-compose-file-not-found",
            path = snapshot.path.display()
        )
    })?;
    serde_yaml::from_slice(content)
        .map_err(|error| crate::tr!("error-compose-parse", error = error))
}

fn revision_for(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn verify_current_revision(path: &Path, expected_revision: &str) -> Result<(), ComposeError> {
    let current = snapshot_file(path).map_err(ComposeError::from)?;
    if current.revision().as_deref() == Some(expected_revision) {
        Ok(())
    } else {
        Err(revision_conflict())
    }
}

pub(crate) fn restore_file(snapshot: &FileSnapshot) -> Result<(), String> {
    restore_file_with_revision(snapshot, None)
}

pub(crate) fn restore_file_if_revision(
    snapshot: &FileSnapshot,
    expected_revision: &str,
) -> Result<(), String> {
    restore_file_with_revision(snapshot, Some(expected_revision))
}

fn restore_file_with_revision(
    snapshot: &FileSnapshot,
    expected_revision: Option<&str>,
) -> Result<(), String> {
    match &snapshot.content {
        Some(content) => {
            let temporary_path = snapshot.path.with_extension("yml.mendimaru.restore.tmp");
            atomic_replace(&snapshot.path, &temporary_path, content, expected_revision)
        }
        None if snapshot.path.exists() => fs::remove_file(&snapshot.path)
            .map_err(|error| crate::tr!("error-compose-save", error = error)),
        None => Ok(()),
    }
}

fn create_backup(snapshot: &FileSnapshot, backup: &Path) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(&snapshot.path)
        .map_err(|error| crate::tr!("error-compose-backup", error = error))?;
    if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
        return Err(crate::tr!(
            "error-compose-backup",
            error = "the Compose file must be a regular file"
        ));
    }
    let content = snapshot.content.as_deref().ok_or_else(|| {
        crate::tr!(
            "error-compose-file-not-found",
            path = snapshot.path.display()
        )
    })?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(source_metadata.permissions().mode() & 0o777);
    }
    let mut backup_file = options
        .open(backup)
        .map_err(|error| crate::tr!("error-compose-backup", error = error))?;
    let result = backup_file
        .write_all(content)
        .and_then(|_| backup_file.sync_all())
        .map_err(|error| crate::tr!("error-compose-backup", error = error));
    if result.is_err() {
        let _ = fs::remove_file(backup);
    }
    result
}

fn atomic_replace(
    path: &Path,
    temporary_path: &Path,
    content: &[u8],
    expected_revision: Option<&str>,
) -> Result<(), String> {
    let result = (|| {
        let target_metadata = fs::symlink_metadata(path)
            .map_err(|error| crate::tr!("error-compose-save", error = error))?;
        if !target_metadata.is_file() || target_metadata.file_type().is_symlink() {
            return Err(crate::tr!(
                "error-compose-save",
                error = "the Compose file must be a regular file"
            ));
        }
        if let Ok(metadata) = fs::symlink_metadata(temporary_path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(crate::tr!(
                    "error-compose-save",
                    error = "the temporary Compose file is unsafe"
                ));
            }
            fs::remove_file(temporary_path)
                .map_err(|error| crate::tr!("error-compose-save", error = error))?;
        }
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(target_metadata.permissions().mode() & 0o777);
        }
        let mut temporary = options
            .open(temporary_path)
            .map_err(|error| crate::tr!("error-compose-save", error = error))?;
        temporary
            .write_all(content)
            .and_then(|_| temporary.sync_all())
            .map_err(|error| crate::tr!("error-compose-save", error = error))?;
        if let Some(expected_revision) = expected_revision {
            verify_current_revision(path, expected_revision).map_err(String::from)?;
        }
        replace_file(temporary_path, path)
            .map_err(|error| crate::tr!("error-compose-save", error = error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    result
}

#[cfg(not(target_os = "windows"))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn winboat_service_name(compose: &Value) -> Result<String, ComposeError> {
    let services = compose
        .get("services")
        .and_then(Value::as_mapping)
        .ok_or_else(|| {
            ComposeError::new(
                ComposeErrorKind::NotWinboat,
                crate::tr!("error-compose-not-winboat"),
            )
        })?;
    let candidates = services
        .iter()
        .filter_map(|(name, service)| {
            let name = name.as_str()?;
            is_winboat_service(service).then(|| name.to_string())
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [name] => Ok(name.clone()),
        [] => Err(ComposeError::new(
            ComposeErrorKind::NotWinboat,
            crate::tr!("error-compose-not-winboat"),
        )),
        _ => Err(ComposeError::new(
            ComposeErrorKind::Ambiguous,
            crate::tr!("error-compose-ambiguous", count = candidates.len()),
        )),
    }
}

fn is_winboat_service(service: &Value) -> bool {
    let Some(mapping) = service.as_mapping() else {
        return false;
    };
    let image_is_windows = mapping
        .get(Value::String("image".to_string()))
        .and_then(Value::as_str)
        .is_some_and(is_dockur_windows_image);
    let volumes = mapping
        .get(Value::String("volumes".to_string()))
        .and_then(Value::as_sequence);
    let has_storage = volumes.is_some_and(|volumes| {
        volumes
            .iter()
            .any(|volume| volume_target_value(volume) == Some("/storage"))
    });
    let container_is_winboat = mapping
        .get(Value::String("container_name".to_string()))
        .and_then(Value::as_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("winboat"));
    let ports = mapping
        .get(Value::String("ports".to_string()))
        .and_then(Value::as_sequence);
    let has_winboat_ports =
        ports.is_some_and(|ports| has_guest_port(ports, 7148) && has_guest_port(ports, 3389));
    let has_winboat_label = mapping
        .get(Value::String("labels".to_string()))
        .is_some_and(value_mentions_winboat);
    let has_winboat_environment = mapping
        .get(Value::String("environment".to_string()))
        .is_some_and(environment_exposes_guest_api);

    image_is_windows
        && has_storage
        && (container_is_winboat
            || has_winboat_ports
            || has_winboat_label
            || has_winboat_environment)
}

fn is_dockur_windows_image(image: &str) -> bool {
    let repository = image
        .trim()
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let repository = match repository.rsplit_once('/') {
        Some((prefix, name)) => {
            let name = name.split(':').next().unwrap_or_default();
            format!("{prefix}/{name}")
        }
        None => repository.split(':').next().unwrap_or_default().to_string(),
    };
    repository == "dockur/windows" || repository.ends_with("/dockur/windows")
}

fn has_guest_port(ports: &[Value], guest_port: u16) -> bool {
    ports
        .iter()
        .filter_map(parse_port_mapping)
        .any(|mapping| mapping.guest_port == guest_port)
}

fn value_mentions_winboat(value: &Value) -> bool {
    match value {
        Value::String(value) => value.to_ascii_lowercase().contains("winboat"),
        Value::Sequence(values) => values.iter().any(value_mentions_winboat),
        Value::Mapping(values) => values
            .iter()
            .any(|(key, value)| value_mentions_winboat(key) || value_mentions_winboat(value)),
        _ => false,
    }
}

fn environment_exposes_guest_api(environment: &Value) -> bool {
    if let Some(mapping) = environment.as_mapping() {
        return mapping.iter().any(|(key, value)| {
            key.as_str().is_some_and(|key| key == "USER_PORTS") && scalar_contains_port(value, 7148)
        });
    }
    environment.as_sequence().is_some_and(|values| {
        values.iter().any(|value| {
            value.as_str().is_some_and(|entry| {
                entry.split_once('=').is_some_and(|(key, value)| {
                    key == "USER_PORTS" && text_contains_port(value, 7148)
                })
            })
        })
    })
}

fn scalar_contains_port(value: &Value, port: u16) -> bool {
    value
        .as_str()
        .is_some_and(|value| text_contains_port(value, port))
        || value.as_u64() == Some(u64::from(port))
}

fn text_contains_port(value: &str, port: u16) -> bool {
    value
        .split(|character: char| !character.is_ascii_digit())
        .any(|part| part.parse::<u16>().ok() == Some(port))
}

pub(crate) fn winboat_compose_service_name(path: &Path) -> Result<String, String> {
    let compose = read_compose(path)?;
    winboat_service_name(&compose).map_err(String::from)
}

fn service_value_named<'a>(compose: &'a Value, service_name: &str) -> Option<&'a Value> {
    compose
        .get("services")?
        .as_mapping()?
        .get(Value::String(service_name.to_string()))
}

fn service_value_named_mut<'a>(
    compose: &'a mut Value,
    service_name: &str,
) -> Option<&'a mut Value> {
    compose
        .get_mut("services")?
        .as_mapping_mut()?
        .get_mut(Value::String(service_name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        apply_shared_mount, compose_file_is_valid, ensure_runtime_port_mapping,
        host_port_for_guest, plan_shared_mount, restore_file, restore_file_if_revision,
        runtime_port_mapping, shared_mount_source, snapshot_file, update_shared_mount,
        ComposeErrorKind,
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
    fn adds_only_a_fixed_loopback_runtime_mapping_and_preserves_storage() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - winboat-data:/storage\n      - /workspace:/shared\n    ports:\n      - 127.0.0.1:47280:7148\n      - 0.0.0.0:8080:8080\nvolumes:\n  winboat-data: {}\n",
        )
        .expect("write compose");

        assert!(ensure_runtime_port_mapping(&compose, 8080).expect("add mapping"));
        let updated = fs::read_to_string(&compose).expect("read compose");
        assert!(updated.contains("winboat-data:/storage"));
        assert!(updated.contains("127.0.0.1:8080:8080/tcp"));
        assert!(!updated.contains("0.0.0.0:8080:8080"));
        assert!(!ensure_runtime_port_mapping(&compose, 8080).expect("mapping is stable"));
        let mapping = runtime_port_mapping(&compose, 8080)
            .expect("inspect mapping")
            .expect("mapping exists");
        assert_eq!(mapping.host_ip, "127.0.0.1");
        assert_eq!(mapping.host_port, Some(8080));
        assert_eq!(mapping.guest_port, 8080);
    }

    #[test]
    fn refuses_host_networking_or_a_compose_without_protected_storage() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    network_mode: host\n    volumes:\n      - data:/storage\n",
        )
        .expect("write compose");
        assert!(ensure_runtime_port_mapping(&compose, 8080)
            .expect_err("host networking rejected")
            .contains("host networking"));

        fs::write(
            &compose,
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /workspace:/shared\n",
        )
        .expect("write compose");
        assert!(ensure_runtime_port_mapping(&compose, 8080).is_err());
    }

    #[test]
    fn updates_only_shared_volume_and_creates_backup() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /old:/shared\n      - data:/storage\n",
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
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - ${HOME}:/shared\n      - data:/storage\n",
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
        fs::write(&compose, "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - data:/storage\n")
            .expect("write valid compose");
        assert!(compose_file_is_valid(&compose.to_string_lossy()));

        fs::write(&compose, "name: parsed-but-not-compose\n").expect("write invalid compose");
        assert!(!compose_file_is_valid(&compose.to_string_lossy()));

        fs::write(&compose, "services: [unterminated\n").expect("write malformed compose");
        assert!(!compose_file_is_valid(&compose.to_string_lossy()));
    }

    #[test]
    fn identifies_official_docker_and_podman_compose_fixtures() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        for fixture in [
            "name: winboat\nservices:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    environment:\n      USER_PORTS: '7148,7147'\n    ports:\n      - 127.0.0.1:47280-47289:7148\n      - 127.0.0.1:47300-47309:3389/tcp\n    volumes:\n      - data:/storage\n      - ${HOME}:/shared\n",
            "name: winboat\nservices:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    environment:\n      - USER_PORTS=7148,7147\n    ports:\n      - target: 7148\n        host_ip: 127.0.0.1\n      - target: 3389\n        protocol: tcp\n    volumes:\n      - type: volume\n        source: data\n        target: /storage\n      - type: bind\n        source: ${HOME}\n        target: /shared\n",
        ] {
            fs::write(&compose, fixture).expect("write official fixture");
            assert!(compose_file_is_valid(&compose.to_string_lossy()));
            let plan = plan_shared_mount(&compose, "/new").expect("WinBoat plan");
            assert_eq!(plan.service_name(), "windows");
        }
    }

    #[test]
    fn rejects_name_only_and_first_service_fallbacks_with_typed_errors() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        for fixture in [
            "services:\n  windows:\n    image: nginx:latest\n    volumes:\n      - data:/storage\n",
            "services:\n  app:\n    image: postgres:18\n    volumes:\n      - data:/var/lib/postgresql/data\n",
        ] {
            fs::write(&compose, fixture).expect("write ordinary Compose");
            assert!(!compose_file_is_valid(&compose.to_string_lossy()));
            assert_eq!(
                plan_shared_mount(&compose, "/new")
                    .expect_err("ordinary Compose rejected")
                    .kind(),
                ComposeErrorKind::NotWinboat
            );
        }
    }

    #[test]
    fn rejects_multiple_strong_candidates_as_ambiguous() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        fs::write(
            &compose,
            "services:\n  primary:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - primary:/storage\n  secondary:\n    image: ghcr.io/dockur/windows:6.03\n    labels:\n      io.winboat.managed: 'true'\n    volumes:\n      - secondary:/storage\n",
        )
        .expect("write ambiguous Compose");

        assert!(!compose_file_is_valid(&compose.to_string_lossy()));
        assert_eq!(
            plan_shared_mount(&compose, "/new")
                .expect_err("ambiguous Compose rejected")
                .kind(),
            ComposeErrorKind::Ambiguous
        );
    }

    #[test]
    fn updates_only_the_identified_service_and_preserves_long_mount_semantics() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        fs::write(
            &compose,
            "services:\n  database:\n    image: postgres:18\n    environment:\n      KEEP: sidecar\n  winboat-vm:\n    image: ghcr.io/dockur/windows:6.03\n    labels:\n      io.winboat.managed: 'true'\n    environment:\n      KEEP: windows\n    networks: [vmnet]\n    volumes:\n      - type: bind\n        source: ${HOME}/old\n        target: /shared\n        read_only: true\n      - type: volume\n        source: winboat-data\n        target: /storage\n      - cache:/cache:ro\nnetworks:\n  vmnet: {}\nvolumes:\n  winboat-data: {}\n  cache: {}\n",
        )
        .expect("write multi-service Compose");
        let before: Value =
            serde_yaml::from_str(&fs::read_to_string(&compose).expect("read original Compose"))
                .expect("parse original Compose");

        let plan = plan_shared_mount(&compose, "/new/workspace").expect("mount plan");
        assert_eq!(plan.service_name(), "winboat-vm");
        assert_eq!(plan.current_source(), Some("${HOME}/old"));
        let applied = apply_shared_mount(plan).expect("apply mount");
        assert!(applied.changed);

        let after: Value =
            serde_yaml::from_str(&fs::read_to_string(&compose).expect("read updated Compose"))
                .expect("parse updated Compose");
        assert_eq!(
            before["services"]["database"],
            after["services"]["database"]
        );
        assert_eq!(
            before["services"]["winboat-vm"]["environment"],
            after["services"]["winboat-vm"]["environment"]
        );
        assert_eq!(
            before["services"]["winboat-vm"]["networks"],
            after["services"]["winboat-vm"]["networks"]
        );
        let volumes = after["services"]["winboat-vm"]["volumes"]
            .as_sequence()
            .expect("updated volumes");
        let shared = volumes
            .iter()
            .find(|volume| super::volume_target_value(volume) == Some("/shared"))
            .expect("shared mount");
        assert_eq!(shared["source"].as_str(), Some("/new/workspace"));
        assert_eq!(shared["read_only"].as_bool(), Some(true));
        let storage_before = super::storage_mounts(
            before["services"]["winboat-vm"]["volumes"]
                .as_sequence()
                .expect("original volumes"),
        );
        assert_eq!(super::storage_mounts(volumes), storage_before);
    }

    #[test]
    fn preserves_short_mount_flags_when_replacing_only_the_source() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        fs::write(
            &compose,
            "services:\n  vm:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /old:/shared:ro\n      - data:/storage\n",
        )
        .expect("write short mount fixture");

        assert!(update_shared_mount(&compose, "/new").expect("update mount"));
        assert!(fs::read_to_string(compose)
            .expect("read Compose")
            .contains("/new:/shared:ro"));
    }

    #[test]
    fn concurrent_edit_conflict_preserves_the_external_content() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        let original = "services:\n  vm:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /old:/shared\n      - data:/storage\n";
        fs::write(&compose, original).expect("write original Compose");
        let plan = plan_shared_mount(&compose, "/new").expect("mount plan");
        let external = format!("{original}# external edit\n");
        fs::write(&compose, &external).expect("inject concurrent edit");

        let error = apply_shared_mount(plan).expect_err("conflict rejected");
        assert_eq!(error.kind(), ComposeErrorKind::RevisionConflict);
        assert_eq!(
            fs::read_to_string(compose).expect("read conflict"),
            external
        );
    }

    #[test]
    fn guarded_rollback_does_not_overwrite_a_later_external_edit() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("compose.yml");
        fs::write(
            &compose,
            "services:\n  vm:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /old:/shared\n      - data:/storage\n",
        )
        .expect("write original Compose");
        let applied = apply_shared_mount(plan_shared_mount(&compose, "/new").expect("mount plan"))
            .expect("apply mount");
        fs::write(&compose, "external edit after apply\n").expect("external edit");

        assert!(restore_file_if_revision(&applied.original, &applied.applied_revision).is_err());
        assert_eq!(
            fs::read_to_string(compose).expect("read external edit"),
            "external edit after apply\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_and_oversized_compose_files() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temp dir");
        let target = temporary.path().join("target.yml");
        let compose = temporary.path().join("docker-compose.yml");
        fs::write(&target, "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - data:/storage\n").expect("target compose");
        symlink(&target, &compose).expect("compose symlink");
        assert!(!compose_file_is_valid(&compose.to_string_lossy()));
        assert!(snapshot_file(&compose).is_err());

        fs::remove_file(&compose).expect("remove compose symlink");
        fs::write(&compose, vec![b'x'; super::MAX_COMPOSE_BYTES as usize + 1])
            .expect("oversized compose");
        assert!(!compose_file_is_valid(&compose.to_string_lossy()));
        assert!(snapshot_file(&compose).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_compose_temporary_and_backup_files() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temp dir");
        let compose = temporary.path().join("docker-compose.yml");
        let victim = temporary.path().join("victim");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /old:/shared\n      - data:/storage\n",
        )
        .expect("compose");
        fs::write(&victim, "unchanged").expect("victim");

        let write_link = compose.with_extension("yml.mendimaru.tmp");
        symlink(&victim, &write_link).expect("write symlink");
        assert!(update_shared_mount(&compose, "/new").is_err());
        assert_eq!(fs::read_to_string(&victim).expect("victim"), "unchanged");

        let backup_link = compose.with_extension("yml.mendimaru.bak");
        fs::remove_file(&backup_link).expect("remove created backup");
        symlink(&victim, &backup_link).expect("backup symlink");
        assert!(update_shared_mount(&compose, "/new").is_err());
        assert_eq!(fs::read_to_string(&victim).expect("victim"), "unchanged");
    }
}
