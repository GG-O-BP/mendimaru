//! Host-wide Runtime port ownership registry for one WinBoat VM. See
//! docs/winboat-multi-runtime.md. The registry is keyed by the same management
//! identity as `vm_use`, lives in a fixed `/tmp/mendimaru-vm-runtime-<uid>`
//! namespace, and deliberately ignores cache, Compose path, and TMPDIR
//! overrides: Runtime sessions recorded in different caches must still see
//! each other's port ownership for the same VM.
use crate::models::AppConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const BUSY: &str =
    "WinBoat Runtime port registry is locked by another participant; retry after it finishes";
pub(crate) const UNTRUSTED: &str =
    "WinBoat Runtime port registry could not be verified; check its ownership and permissions";

const SCHEMA_VERSION: u32 = 1;

/// How the record file referenced by a registry entry looks right now.
/// Records are owned by the Runtime module; the registry only reconciles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordLiveness {
    Active,
    Stopped,
    Gone,
}

pub(crate) type Probe = fn(&Path) -> RecordLiveness;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreservedMapping {
    pub(crate) host_ip: String,
    pub(crate) host_port: Option<u16>,
    pub(crate) guest_port: u16,
    pub(crate) protocol: String,
}

impl From<&crate::config::RuntimePortMapping> for PreservedMapping {
    fn from(mapping: &crate::config::RuntimePortMapping) -> Self {
        Self {
            host_ip: mapping.host_ip.clone(),
            host_port: mapping.host_port,
            guest_port: mapping.guest_port,
            protocol: mapping.protocol.clone(),
        }
    }
}

impl PreservedMapping {
    pub(crate) fn to_mapping(&self) -> crate::config::RuntimePortMapping {
        crate::config::RuntimePortMapping {
            host_ip: self.host_ip.clone(),
            host_port: self.host_port,
            guest_port: self.guest_port,
            protocol: self.protocol.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PortEntry {
    pub(crate) session_id: String,
    pub(crate) guest_port: u16,
    pub(crate) host_port: u16,
    pub(crate) record_path: String,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) stopped_at: Option<DateTime<Utc>>,
    /// Forwarding that existed on this guest port before Mendimaru took the
    /// port over. Restored when the port's ownership is finally cleaned up.
    pub(crate) pre_existing: Vec<PreservedMapping>,
}

impl PortEntry {
    pub(crate) const fn is_live(&self) -> bool {
        self.stopped_at.is_none()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistryFile {
    schema_version: u32,
    entries: Vec<PortEntry>,
}

#[derive(Debug)]
pub(crate) enum RegistryError {
    /// Another live entry owns the same guest port on this VM.
    Conflict(PortEntry),
    Busy,
    Untrusted,
}

impl RegistryError {
    pub(crate) fn conflicting_entry(&self) -> Option<&PortEntry> {
        match self {
            Self::Conflict(entry) => Some(entry),
            _ => None,
        }
    }

    pub(crate) const fn is_busy(&self) -> bool {
        matches!(self, Self::Busy)
    }
}

/// A reserved entry that releases ownership unless explicitly committed.
#[derive(Debug)]
pub(crate) struct Reservation {
    paths: (PathBuf, PathBuf),
    session_id: String,
    committed: bool,
}

impl Reservation {
    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        linux::update_at(&self.paths, |entries| {
            entries.retain(|entry| entry.session_id != self.session_id);
            Ok(())
        })
        .ok();
    }
}

/// Atomically reserve a guest port for a new Runtime session. Must be called
/// while holding the exclusive VM use lease; the registry lock makes the
/// read-modify-write safe for every other reader.
pub(crate) fn reserve(
    config: &AppConfig,
    entry: PortEntry,
    probe: Probe,
) -> Result<Reservation, RegistryError> {
    linux::paths(config).and_then(|paths| reserve_at(&paths, entry, probe))
}

/// Reconciled entries for this VM. Crashed writers (record gone) and records
/// that say Stopped are folded in before the entries are returned.
pub(crate) fn snapshot(config: &AppConfig, probe: Probe) -> Result<Vec<PortEntry>, RegistryError> {
    linux::paths(config).and_then(|paths| snapshot_at(&paths, probe))
}

/// Record that a session stopped but its forwarding removal is deferred while
/// other sessions are still live on this VM.
pub(crate) fn mark_stopped(config: &AppConfig, session_id: &str) -> Result<(), RegistryError> {
    linux::paths(config).and_then(|paths| mark_stopped_at(&paths, session_id))
}

/// Drop entries whose forwarding was actually removed from the Compose file.
pub(crate) fn remove(config: &AppConfig, session_ids: &[&str]) -> Result<(), RegistryError> {
    linux::paths(config).and_then(|paths| remove_at(&paths, session_ids))
}

#[cfg(target_os = "linux")]
fn reconcile(entries: &mut Vec<PortEntry>, probe: Probe) {
    entries.retain(|entry| probe(Path::new(&entry.record_path)) != RecordLiveness::Gone);
    let now = Utc::now();
    for entry in entries {
        if entry.stopped_at.is_none()
            && probe(Path::new(&entry.record_path)) == RecordLiveness::Stopped
        {
            entry.stopped_at = Some(now);
        }
    }
}

#[cfg(target_os = "linux")]
fn reserve_at(
    paths: &(PathBuf, PathBuf),
    entry: PortEntry,
    probe: Probe,
) -> Result<Reservation, RegistryError> {
    let session_id = entry.session_id.clone();
    linux::update_at(paths, |entries| {
        reconcile(entries, probe);
        if let Some(conflict) = entries
            .iter()
            .find(|existing| {
                existing.is_live()
                    && (existing.guest_port == entry.guest_port
                        || existing.record_path == entry.record_path)
            })
            .cloned()
        {
            return Err(RegistryError::Conflict(conflict));
        }
        entries.retain(|existing| existing.session_id != entry.session_id);
        entries.push(entry.clone());
        Ok(())
    })?;
    Ok(Reservation {
        paths: paths.clone(),
        session_id,
        committed: false,
    })
}

#[cfg(target_os = "linux")]
fn snapshot_at(paths: &(PathBuf, PathBuf), probe: Probe) -> Result<Vec<PortEntry>, RegistryError> {
    linux::update_at(paths, |entries| {
        reconcile(entries, probe);
        Ok(())
    })?;
    linux::read_at(paths)
}

#[cfg(target_os = "linux")]
fn mark_stopped_at(paths: &(PathBuf, PathBuf), session_id: &str) -> Result<(), RegistryError> {
    linux::update_at(paths, |entries| {
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.session_id == session_id)
        {
            entry.stopped_at.get_or_insert_with(Utc::now);
        }
        Ok(())
    })
}

#[cfg(target_os = "linux")]
fn remove_at(paths: &(PathBuf, PathBuf), session_ids: &[&str]) -> Result<(), RegistryError> {
    linux::update_at(paths, |entries| {
        entries.retain(|entry| !session_ids.contains(&entry.session_id.as_str()));
        Ok(())
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use cap_std::fs::{OpenOptions, OpenOptionsExt, PermissionsExt};
    use std::fs::File;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(3);
    const POLL: Duration = Duration::from_millis(25);
    const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;

    fn root() -> Result<PathBuf, RegistryError> {
        let uid = unsafe { libc::geteuid() };
        // A fixed local namespace: TMPDIR/XDG/cache overrides cannot split it.
        let path = std::path::PathBuf::from(format!("/tmp/mendimaru-vm-runtime-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(RegistryError::Untrusted),
        }
        let directory =
            super::super::nvram::open_directory(&path).map_err(|_| RegistryError::Untrusted)?;
        if directory
            .dir_metadata()
            .map_err(|_| RegistryError::Untrusted)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(RegistryError::Untrusted);
        }
        Ok(path)
    }

    pub(super) fn paths(config: &AppConfig) -> Result<(PathBuf, PathBuf), RegistryError> {
        let key =
            crate::winboat::vm_use::identity_key(config).map_err(|_| RegistryError::Untrusted)?;
        let base = root()?;
        let base = std::fs::canonicalize(&base).map_err(|_| RegistryError::Untrusted)?;
        Ok((
            base.join(format!("{key}.json")),
            base.join(format!("{key}.lock")),
        ))
    }

    fn trusted_file_name(path: &Path) -> Result<&str, RegistryError> {
        path.file_name()
            .and_then(|name| name.to_str())
            .filter(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
            })
            .ok_or(RegistryError::Untrusted)
    }

    fn open_lock(directory: &cap_std::fs::Dir, name: &str) -> Result<File, RegistryError> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = directory
            .open_with(name, &options)
            .map_err(|_| RegistryError::Untrusted)?
            .into_std();
        let metadata = file.metadata().map_err(|_| RegistryError::Untrusted)?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
        {
            return Err(RegistryError::Untrusted);
        }
        Ok(file)
    }

    fn read_file(path: &Path) -> Result<Vec<PortEntry>, RegistryError> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            // A fresh VM has no registry file yet; the lock file is the anchor.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(RegistryError::Untrusted),
        };
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.len() > MAX_REGISTRY_BYTES
        {
            return Err(RegistryError::Untrusted);
        }
        let bytes = std::fs::read(path).map_err(|_| RegistryError::Untrusted)?;
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let file: RegistryFile =
            serde_json::from_slice(&bytes).map_err(|_| RegistryError::Untrusted)?;
        if file.schema_version != SCHEMA_VERSION {
            return Err(RegistryError::Untrusted);
        }
        Ok(file.entries)
    }

    fn write_file(path: &Path, entries: &[PortEntry]) -> Result<(), RegistryError> {
        let file = RegistryFile {
            schema_version: SCHEMA_VERSION,
            entries: entries.to_vec(),
        };
        let bytes = serde_json::to_vec(&file).map_err(|_| RegistryError::Untrusted)?;
        let parent = path.parent().ok_or(RegistryError::Untrusted)?;
        let directory =
            super::super::nvram::open_directory(parent).map_err(|_| RegistryError::Untrusted)?;
        let temporary = format!("{}.tmp", trusted_file_name(path)?);
        {
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
            let mut handle = directory
                .open_with(&temporary, &options)
                .map_err(|_| RegistryError::Untrusted)?
                .into_std();
            use std::io::Write as _;
            handle
                .write_all(&bytes)
                .map_err(|_| RegistryError::Untrusted)?;
            handle.sync_all().map_err(|_| RegistryError::Untrusted)?;
        }
        std::fs::rename(parent.join(&temporary), path).map_err(|_| RegistryError::Untrusted)?;
        Ok(())
    }

    fn wait_for_lock(file: &File, wait: Duration) -> Result<(), RegistryError> {
        let deadline = Instant::now() + wait;
        loop {
            match fs2::FileExt::try_lock_exclusive(file) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(RegistryError::Untrusted),
            }
            if Instant::now() >= deadline {
                return Err(RegistryError::Busy);
            }
            std::thread::sleep(POLL);
        }
    }

    pub(super) fn update_at(
        paths: &(PathBuf, PathBuf),
        update: impl FnOnce(&mut Vec<PortEntry>) -> Result<(), RegistryError>,
    ) -> Result<(), RegistryError> {
        let directory =
            super::super::nvram::open_directory(paths.0.parent().unwrap_or(Path::new("/")))
                .map_err(|_| RegistryError::Untrusted)?;
        let lock = open_lock(&directory, trusted_file_name(&paths.1)?)?;
        // The exclusive lock is held for the whole read-modify-write; the
        // descriptor stays open until the new content is durably in place.
        wait_for_lock(&lock, WAIT)?;
        let mut entries = read_file(&paths.0)?;
        update(&mut entries)?;
        write_file(&paths.0, &entries)
    }

    pub(super) fn read_at(paths: &(PathBuf, PathBuf)) -> Result<Vec<PortEntry>, RegistryError> {
        let directory =
            super::super::nvram::open_directory(paths.0.parent().unwrap_or(Path::new("/")))
                .map_err(|_| RegistryError::Untrusted)?;
        let lock = open_lock(&directory, trusted_file_name(&paths.1)?)?;
        wait_for_lock(&lock, WAIT)?;
        read_file(&paths.0)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    // Probes are plain function pointers in production. Tests drive the same
    // shape by making each record's bytes state its liveness, so transitions
    // are simulated by rewriting record content.
    fn content_probe(path: &Path) -> RecordLiveness {
        match std::fs::read(path) {
            Ok(content) if content == b"active" => RecordLiveness::Active,
            Ok(content) if content == b"stopped" => RecordLiveness::Stopped,
            _ => RecordLiveness::Gone,
        }
    }

    fn entry(session_id: &str, port: u16, record: &Path) -> PortEntry {
        PortEntry {
            session_id: session_id.to_string(),
            guest_port: port,
            host_port: port,
            record_path: record.to_string_lossy().to_string(),
            started_at: Utc::now(),
            stopped_at: None,
            pre_existing: Vec::new(),
        }
    }

    fn record_directory(root: &Path, name: &str) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).expect("record directory");
        directory.join("session.json")
    }

    #[test]
    fn reserve_conflict_reconciliation_and_release() {
        let directory = tempfile::tempdir().expect("registry directory");
        let paths = (
            directory.path().join("vm.json"),
            directory.path().join("vm.lock"),
        );
        // Cross-cache ownership: both records live outside the registry root.
        let a_path = record_directory(directory.path(), "cache-a");
        let b_path = record_directory(directory.path(), "cache-b");
        std::fs::write(&a_path, b"active").expect("record a");

        let first = reserve_at(&paths, entry("runtime_1", 8080, &a_path), content_probe)
            .expect("first reserve");
        assert_eq!(
            snapshot_at(&paths, content_probe).expect("snapshot").len(),
            1
        );

        // A second session on the same port conflicts atomically.
        std::fs::write(&b_path, b"active").expect("record b");
        let conflict = reserve_at(&paths, entry("runtime_2", 8080, &b_path), content_probe)
            .expect_err("duplicate live port");
        assert_eq!(
            conflict.conflicting_entry().expect("conflict").session_id,
            "runtime_1"
        );

        // A different port reserves cleanly.
        reserve_at(&paths, entry("runtime_2", 18080, &b_path), content_probe)
            .expect("different port reserves");

        // An uncommitted reservation releases its ownership on drop.
        drop(first);
        let after = snapshot_at(&paths, content_probe).expect("snapshot after drop");
        assert!(
            after.iter().all(|entry| entry.session_id != "runtime_1"),
            "uncommitted ownership must be released"
        );
    }

    #[test]
    fn crashed_and_stopped_records_are_reconciled() {
        let directory = tempfile::tempdir().expect("registry directory");
        let paths = (
            directory.path().join("vm.json"),
            directory.path().join("vm.lock"),
        );
        let gone = record_directory(directory.path(), "gone");
        let stopped = record_directory(directory.path(), "stopped");
        let live = record_directory(directory.path(), "live");
        std::fs::write(&gone, b"active").expect("gone record");
        std::fs::write(&stopped, b"stopped").expect("stopped record");
        std::fs::write(&live, b"active").expect("live record");

        let mut reservation = reserve_at(&paths, entry("runtime_gone", 8080, &gone), content_probe)
            .expect("reserve gone");
        reservation.commit();
        let mut reservation = reserve_at(
            &paths,
            entry("runtime_stopped", 8081, &stopped),
            content_probe,
        )
        .expect("reserve stopped");
        reservation.commit();
        let mut reservation = reserve_at(&paths, entry("runtime_live", 8082, &live), content_probe)
            .expect("reserve live");
        reservation.commit();

        // Simulate a crash: the record file disappeared while marked live.
        std::fs::remove_file(&gone).expect("crash removes record");
        let snapshot = snapshot_at(&paths, content_probe).expect("reconciled snapshot");
        assert!(
            snapshot
                .iter()
                .all(|entry| entry.session_id != "runtime_gone"),
            "a record deleted by a crash must not keep owning a port"
        );
        let stopped_entry = snapshot
            .iter()
            .find(|entry| entry.session_id == "runtime_stopped")
            .expect("stopped entry retained until cleanup");
        assert!(stopped_entry.stopped_at.is_some());

        mark_stopped_at(&paths, "runtime_live").expect("mark live stopped");
        let snapshot = snapshot_at(&paths, content_probe).expect("snapshot");
        assert!(snapshot
            .iter()
            .find(|entry| entry.session_id == "runtime_live")
            .expect("live entry")
            .stopped_at
            .is_some());

        remove_at(&paths, &["runtime_stopped", "runtime_live"]).expect("remove cleaned");
        assert!(snapshot_at(&paths, content_probe)
            .expect("snapshot after remove")
            .is_empty());
    }

    #[test]
    fn corrupt_or_future_registry_content_is_refused() {
        let directory = tempfile::tempdir().expect("registry directory");
        let paths = (
            directory.path().join("vm.json"),
            directory.path().join("vm.lock"),
        );
        std::fs::write(&paths.0, b"{\"schemaVersion\":1,\"entries\":").expect("corrupt");
        let error = snapshot_at(&paths, content_probe).expect_err("corrupt registry");
        assert!(error.conflicting_entry().is_none());
        assert!(!error.is_busy());

        std::fs::write(&paths.0, b"{\"schemaVersion\":2,\"entries\":[]}").expect("future schema");
        assert!(snapshot_at(&paths, content_probe).is_err());

        std::fs::remove_file(&paths.0).expect("reset");
        assert!(snapshot_at(&paths, content_probe)
            .expect("fresh registry")
            .is_empty());
    }
}
