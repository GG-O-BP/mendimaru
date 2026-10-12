//! Explicit, inode-bound permission repair of Mendimaru coordination entries.
//! Never repairs Compose/NVRAM directories, traverses links, recursively chmods,
//! unlinks a lock, or changes another UID's files.
use super::{linux, LockError};
use crate::contracts::{BackendError, BackendErrorCode, BackendId, CapabilityId};
use crate::models::{AppConfig, CommandError};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use serde::Serialize;
use std::fs::File;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub(crate) id: String,
    pub(crate) directory: String,
    pub(crate) files: Vec<String>,
}
struct Entry {
    directory: Dir,
    name: String,
    file: File,
    identity: (u64, u64, u32),
}
struct Plan {
    preview: Preview,
    root: PathBuf,
    directory: Dir,
    identity: (u64, u64, u32),
    entries: Vec<Entry>,
    vm_key: String,
    compose_file: String,
    created: Instant,
}
static PENDING: Mutex<Option<Plan>> = Mutex::new(None);

fn error(lock: LockError) -> CommandError {
    lock.command_error(CapabilityId::EnvironmentConfigure)
}
fn changed() -> CommandError {
    let mut error = BackendError::operation(
        BackendId::LinuxWinboat,
        CapabilityId::EnvironmentConfigure,
        crate::tr!("error-coordination-repair-changed"),
    );
    error.code = BackendErrorCode::PreconditionFailed;
    error.into()
}
fn directory_metadata(directory: &Dir) -> std::io::Result<std::fs::Metadata> {
    directory.try_clone()?.into_std_file().metadata()
}
fn identity(metadata: &std::fs::Metadata) -> (u64, u64, u32) {
    (metadata.dev(), metadata.ino(), metadata.mode())
}
fn trusted_entry(directory: Dir, name: &str) -> Result<Entry, LockError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = directory
        .open_with(name, &options)
        .map_err(|e| linux::classify(&e))?
        .into_std();
    let metadata = file.metadata().map_err(|e| linux::classify(&e))?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err(LockError::Untrusted);
    }
    Ok(Entry {
        directory,
        name: name.into(),
        file,
        identity: identity(&metadata),
    })
}
fn known_name(name: &str) -> bool {
    let Some((key, suffix)) = name.split_once('.') else {
        return false;
    };
    key.len() == 64
        && key.bytes().all(|b| b.is_ascii_hexdigit())
        && matches!(suffix, "maintenance.lock" | "runtime-stop.lock")
}
fn plan_at(config: &AppConfig, root: &Path) -> Result<Plan, LockError> {
    super::validate_identity(config)?;
    let vm_key = crate::winboat::vm_use::identity_key(config).map_err(|_| LockError::Identity)?;
    let directory = linux::owned_directory(root).map_err(|e| linux::classify(&e))?;
    let metadata = directory_metadata(&directory).map_err(|e| linux::classify(&e))?;
    let mut entries = Vec::new();
    for entry in directory.entries().map_err(|e| linux::classify(&e))? {
        let name = entry
            .map_err(|e| linux::classify(&e))?
            .file_name()
            .into_string()
            .map_err(|_| LockError::Untrusted)?;
        if entries.len() >= 1024 || !known_name(&name) {
            return Err(LockError::Untrusted);
        }
        entries.push(trusted_entry(
            directory.try_clone().map_err(|e| linux::classify(&e))?,
            &name,
        )?);
    }
    // Legacy bridge files belong to Mendimaru, not to WinBoat. Repair only the
    // two exact names for this selected VM, never their Compose parent folder.
    if let Some(parent) = Path::new(&config.compose_file).parent() {
        let legacy = match linux::owned_directory(parent) {
            Ok(directory) => Some(directory),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(linux::classify(&error)),
        };
        if let Some(legacy) = legacy {
            for name in [
                ".mendimaru-maintenance.lock",
                ".mendimaru-runtime-stop.lock",
            ] {
                match legacy.symlink_metadata(name) {
                    Ok(_) => entries.push(trusted_entry(
                        legacy.try_clone().map_err(|e| linux::classify(&e))?,
                        name,
                    )?),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(linux::classify(&e)),
                }
            }
        }
    }
    let mut files: Vec<_> = entries
        .iter()
        .map(|entry| {
            if entry.name.starts_with('.') {
                Path::new(&config.compose_file)
                    .parent()
                    .unwrap()
                    .join(&entry.name)
                    .to_string_lossy()
                    .into_owned()
            } else {
                root.join(&entry.name).to_string_lossy().into_owned()
            }
        })
        .collect();
    files.sort();
    Ok(Plan {
        preview: Preview {
            id: crate::contracts::secure_identifier("coordination")
                .map_err(|_| LockError::Unavailable)?,
            directory: root.to_string_lossy().into_owned(),
            files,
        },
        root: root.into(),
        directory,
        identity: identity(&metadata),
        entries,
        vm_key,
        compose_file: config.compose_file.clone(),
        created: Instant::now(),
    })
}

pub(crate) fn preview(config: &AppConfig) -> Result<Preview, CommandError> {
    let plan = plan_at(config, &linux::root()).map_err(error)?;
    let preview = plan.preview.clone();
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(plan);
    Ok(preview)
}
impl Plan {
    fn apply(self, config: &AppConfig) -> Result<(), CommandError> {
        super::validate_identity(config).map_err(error)?;
        if self.compose_file != config.compose_file
            || self.created.elapsed() > Duration::from_secs(300)
            || crate::winboat::vm_use::identity_key(config).as_deref() != Ok(self.vm_key.as_str())
        {
            return Err(changed());
        }
        let current = linux::owned_directory(&self.root).map_err(|e| error(linux::classify(&e)))?;
        if identity(&directory_metadata(&current).map_err(|e| error(linux::classify(&e)))?)
            != self.identity
        {
            return Err(changed());
        }
        // Verify the whole reviewed set before the first chmod. A new unknown
        // entry, replaced inode or mode change invalidates the confirmation.
        let names: Vec<_> = current
            .entries()
            .map_err(|e| error(linux::classify(&e)))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()
            .map_err(|e| error(linux::classify(&e)))?;
        if names.len()
            != self
                .entries
                .iter()
                .filter(|e| !e.name.starts_with('.'))
                .count()
            || names
                .iter()
                .any(|name| !self.entries.iter().any(|e| name == e.name.as_str()))
        {
            return Err(changed());
        }
        for entry in &self.entries {
            if entry.name.starts_with('.') {
                let parent = Path::new(&config.compose_file)
                    .parent()
                    .ok_or_else(changed)?;
                let live_directory = linux::owned_directory(parent).map_err(|_| changed())?;
                let live = directory_metadata(&live_directory).map_err(|_| changed())?;
                let reviewed = directory_metadata(&entry.directory).map_err(|_| changed())?;
                if (live.dev(), live.ino()) != (reviewed.dev(), reviewed.ino()) {
                    return Err(changed());
                }
            }
            let metadata = entry
                .directory
                .symlink_metadata(&entry.name)
                .map_err(|_| changed())?;
            use cap_std::fs::MetadataExt;
            use cap_std::fs::PermissionsExt;
            if metadata.dev() != entry.identity.0
                || metadata.ino() != entry.identity.1
                || metadata.permissions().mode() != entry.identity.2
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.nlink() != 1
                || !metadata.is_file()
            {
                return Err(changed());
            }
            linux::try_lock(&entry.file, true).map_err(error)?;
        }
        // Existing descriptors, not pathnames, are the only mutation targets.
        self.directory
            .into_std_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|e| error(linux::classify(&e)))?;
        for entry in &self.entries {
            entry
                .file
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| error(linux::classify(&e)))?;
        }
        Ok(())
    }
}
pub(crate) fn repair(config: &AppConfig, id: &str, confirmed: bool) -> Result<(), CommandError> {
    if !confirmed {
        let mut error = BackendError::operation(
            BackendId::LinuxWinboat,
            CapabilityId::EnvironmentConfigure,
            crate::tr!("error-coordination-repair-confirmation"),
        );
        error.code = BackendErrorCode::InvalidRequest;
        return Err(error.into());
    }
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if pending.as_ref().is_none_or(|p| p.preview.id != id) {
        return Err(changed());
    }
    let plan = pending.take().ok_or_else(changed)?;
    drop(pending);
    plan.apply(config)
}

#[cfg(test)]
mod tests;
