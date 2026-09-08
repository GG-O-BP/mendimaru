//! Opt-in recovery of an erased OVMF variable store, never a generic QEMU repair.
use crate::config::{nvram_mount_plan, NvramMountPlan};
use crate::contracts::BackendErrorCode;
use crate::models::{AppConfig, ContainerStatus};
use crate::process::{self, CommandPolicy};
use cap_std::fs::MetadataExt as _;
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::process::Command;

const TARGET: &str = "windows.vars";
const VARS_BYTES: usize = 540_672;
const PREVIEW_LIFETIME: Duration = Duration::from_secs(300);
static PENDING: Mutex<Option<Plan>> = Mutex::new(None);
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub id: String,
    pub target_path: String,
    pub backup_path: String,
    pub original_path: String,
    pub bytes: usize,
    pub sha256: String,
    pub evidence: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Outcome {
    pub ready: bool,
    pub rolled_back: bool,
    pub rollback_required: bool,
}

struct Plan {
    preview: Preview,
    created: Instant,
    compose_file: String,
    mount: NvramMountPlan,
    container_id: String,
    image_id: String,
    directory: Dir,
    directory_identity: (u64, u64),
    original: Vec<u8>,
    file_identity: (u64, u64),
    backup: String,
    retired: String,
    replaced: bool,
}

fn rejected() -> String {
    crate::tr!("error-nvram-unsupported")
}
fn changed() -> String {
    crate::tr!("error-nvram-preview-changed")
}

/// Walk every component with no-follow directory handles, including ancestors.
pub(super) fn open_directory(path: &Path) -> std::io::Result<Dir> {
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut directory = File::open("/")?;
    let mut depth = 0;
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(name.as_bytes())?;
        // SAFETY: valid directory fd and NUL-terminated component; ownership of
        // the returned descriptor is transferred to File exactly once.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        directory = unsafe { File::from_raw_fd(fd) };
        depth += 1;
    }
    let metadata = directory.metadata()?;
    if depth < 2 || metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    Ok(Dir::from_std_file(directory))
}

fn read_vars(directory: &Dir, name: &str) -> Result<(File, Vec<u8>), String> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = directory
        .open_with(name, &options)
        .map_err(|_| rejected())?
        .into_std();
    let metadata = file.metadata().map_err(|_| rejected())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
        || metadata.len() != VARS_BYTES as u64
    {
        return Err(rejected());
    }
    let mut bytes = Vec::with_capacity(VARS_BYTES);
    (&mut file)
        .take(VARS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| rejected())?;
    if bytes.len() != VARS_BYTES {
        return Err(rejected());
    }
    Ok((file, bytes))
}

fn erased_store(bytes: &[u8]) -> bool {
    bytes.len() == VARS_BYTES && (bytes.iter().all(|b| *b == 0) || bytes.iter().all(|b| *b == 0xff))
}

fn only_target(directory: &Dir) -> Result<(), String> {
    let mut candidates = Vec::new();
    for (index, entry) in directory.entries().map_err(|_| rejected())?.enumerate() {
        if index >= 128 {
            return Err(rejected());
        }
        let name = entry.map_err(|_| rejected())?.file_name();
        if name.to_string_lossy().ends_with(".vars") {
            candidates.push(name);
        }
    }
    if candidates.len() != 1 || candidates[0] != TARGET {
        return Err(rejected());
    }
    Ok(())
}

async fn inspect(config: &AppConfig) -> Result<serde_json::Value, String> {
    let mut command = Command::new(config.container_runtime.as_str());
    command.args(["inspect", &config.container_name]);
    let output = process::output(
        command,
        CommandPolicy::new(Duration::from_secs(2), 64 * 1024),
        None,
        "UEFI recovery preconditions",
    )
    .await
    .map_err(|_| rejected())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(rejected());
    }
    let mut values: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|_| rejected())?;
    if values.len() != 1 {
        return Err(rejected());
    }
    let value = values.remove(0);
    if value
        .pointer("/Config/Labels/com.docker.compose.project.config_files")
        .and_then(|v| v.as_str())
        != Some(config.compose_file.as_str())
    {
        return Err(rejected());
    }
    Ok(value)
}

fn validate_container(
    value: &serde_json::Value,
    mount: &NvramMountPlan,
    require_stopped: bool,
) -> Result<(String, String), String> {
    let status = value
        .pointer("/State/Status")
        .and_then(|v| v.as_str())
        .map(ContainerStatus::from_runtime)
        .ok_or_else(rejected)?;
    if require_stopped
        && (!matches!(status, ContainerStatus::Exited | ContainerStatus::Created)
            || ["Running", "Restarting", "Paused"].iter().any(|flag| {
                value
                    .get("State")
                    .and_then(|v| v.get(flag))
                    .and_then(|v| v.as_bool())
                    != Some(false)
            }))
    {
        return Err(crate::tr!("error-nvram-not-stopped"));
    }
    if value
        .pointer("/Config/Labels/com.docker.compose.service")
        .and_then(|v| v.as_str())
        != Some(mount.service.as_str())
        || value.pointer("/Config/Image").and_then(|v| v.as_str()) != Some(mount.image.as_str())
    {
        return Err(rejected());
    }
    let mounts = value
        .get("Mounts")
        .and_then(|v| v.as_array())
        .ok_or_else(rejected)?;
    let storage = mounts
        .iter()
        .filter(|m| {
            m.get("Destination")
                .and_then(|v| v.as_str())
                .is_some_and(|d| d == "/storage" || d.starts_with("/storage/"))
        })
        .collect::<Vec<_>>();
    let [storage] = storage.as_slice() else {
        return Err(rejected());
    };
    if storage.get("Type").and_then(|v| v.as_str()) != Some("bind")
        || storage.get("RW").and_then(|v| v.as_bool()) != Some(true)
        || storage.get("Source").and_then(|v| v.as_str()) != mount.directory.to_str()
    {
        return Err(rejected());
    }
    let environment = value
        .pointer("/Config/Env")
        .and_then(|v| v.as_array())
        .ok_or_else(rejected)?;
    if environment.iter().any(|v| !v.is_string()) {
        return Err(rejected());
    }
    for key in ["BOOT_MODE", "STORAGE", "BIOS", "CLEAR"] {
        let values = environment
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(|v| v.split_once('='))
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        if values.len() > 1 {
            return Err(rejected());
        }
        let value = values.first().copied().unwrap_or("");
        if !match key {
            "BOOT_MODE" => value.is_empty() || value == "windows",
            "STORAGE" => value.is_empty() || value == "/storage",
            "CLEAR" => matches!(value, "" | "N" | "0" | "false"),
            _ => value.is_empty(),
        } {
            return Err(rejected());
        }
    }
    let id = value
        .get("Id")
        .and_then(|v| v.as_str())
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(rejected)?;
    let image = value
        .get("Image")
        .and_then(|v| v.as_str())
        .filter(|v| {
            v.starts_with("sha256:")
                && v.len() == 71
                && v[7..].bytes().all(|b| b.is_ascii_hexdigit())
        })
        .ok_or_else(rejected)?;
    Ok((id.to_string(), image.to_string()))
}

async fn plan(config: &AppConfig) -> Result<Plan, String> {
    if !super::startup::snapshot(config).is_some_and(|a| {
        a.phase == super::startup::StartupPhase::StartupFailed
            && a.error_code == Some(BackendErrorCode::QemuBootTimeout)
    }) {
        return Err(rejected());
    }
    let mount = nvram_mount_plan(config)?;
    let (container_id, image_id) = validate_container(&inspect(config).await?, &mount, true)?;
    let directory = open_directory(&mount.directory).map_err(|_| rejected())?;
    only_target(&directory)?;
    let (file, original) = read_vars(&directory, TARGET)?;
    if !erased_store(&original) {
        return Err(rejected());
    }
    let metadata = file.metadata().map_err(|_| rejected())?;
    let directory_metadata = directory.dir_metadata().map_err(|_| rejected())?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| rejected())?;
    let id = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let backup = format!("windows.vars.mendimaru-{stamp}-{id}.bak");
    let retired = format!("windows.vars.mendimaru-{stamp}-{id}.original");
    let preview = Preview {
        id,
        target_path: mount.directory.join(TARGET).to_string_lossy().into_owned(),
        backup_path: mount.directory.join(&backup).to_string_lossy().into_owned(),
        original_path: mount
            .directory
            .join(&retired)
            .to_string_lossy()
            .into_owned(),
        bytes: original.len(),
        sha256: format!("{:x}", Sha256::digest(&original)),
        evidence: "erased-ovmf-variable-store",
    };
    Ok(Plan {
        preview,
        created: Instant::now(),
        compose_file: config.compose_file.clone(),
        mount,
        container_id,
        image_id,
        directory,
        directory_identity: (directory_metadata.dev(), directory_metadata.ino()),
        original,
        file_identity: (metadata.dev(), metadata.ino()),
        backup,
        retired,
        replaced: false,
    })
}

pub(crate) async fn available(config: &AppConfig) -> bool {
    plan(config).await.is_ok()
}

pub(crate) async fn preview(config: &AppConfig) -> Result<Preview, String> {
    let _lease = super::maintenance::acquire(config, true)?;
    let plan = plan(config).await?;
    let preview = plan.preview.clone();
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if pending.as_ref().is_some_and(|p| p.replaced) {
        return Err(crate::tr!("error-nvram-rollback-required"));
    }
    *pending = Some(plan);
    Ok(preview)
}

impl Plan {
    async fn revalidate(&self, config: &AppConfig, stopped: bool) -> Result<(), String> {
        if config.compose_file != self.compose_file {
            return Err(changed());
        }
        let mount = nvram_mount_plan(config).map_err(|_| changed())?;
        if mount.revision != self.mount.revision || mount.directory != self.mount.directory {
            return Err(changed());
        }
        let (id, image) = validate_container(&inspect(config).await?, &mount, stopped)?;
        if id != self.container_id || image != self.image_id {
            return Err(changed());
        }
        let directory = open_directory(&mount.directory).map_err(|_| changed())?;
        let metadata = directory.dir_metadata().map_err(|_| changed())?;
        if (metadata.dev(), metadata.ino()) != self.directory_identity {
            return Err(changed());
        }
        Ok(())
    }

    fn backup_and_retire(&mut self) -> Result<(), String> {
        only_target(&self.directory)?;
        let (file, bytes) = read_vars(&self.directory, TARGET)?;
        let metadata = file.metadata().map_err(|_| changed())?;
        if bytes != self.original || (metadata.dev(), metadata.ino()) != self.file_identity {
            return Err(changed());
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        let mut backup = self
            .directory
            .open_with(&self.backup, &options)
            .map_err(|_| changed())?;
        backup
            .write_all(&self.original)
            .and_then(|_| backup.sync_all())
            .map_err(|_| changed())?;
        sync_directory(&self.directory)?;
        rename_new(&self.directory, TARGET, &self.retired)?;
        self.replaced = true;
        sync_directory(&self.directory)?;
        let (retired, bytes) = read_vars(&self.directory, &self.retired)?;
        let metadata = retired.metadata().map_err(|_| changed())?;
        if bytes != self.original || (metadata.dev(), metadata.ino()) != self.file_identity {
            return Err(changed());
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<(), String> {
        let (_, backup) = read_vars(&self.directory, &self.backup)?;
        if backup != self.original {
            return Err(changed());
        }
        let (_, retired) = read_vars(&self.directory, &self.retired)?;
        if retired != self.original {
            return Err(changed());
        }
        match self.directory.symlink_metadata(TARGET) {
            Ok(_) => {
                // Preserve the failed regenerated store as evidence; never overwrite it.
                read_vars(&self.directory, TARGET)?;
                rename_new(&self.directory, TARGET, &format!("{}.failed", self.backup))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(changed()),
        }
        rename_new(&self.directory, &self.retired, TARGET)?;
        sync_directory(&self.directory)?;
        self.replaced = false;
        Ok(())
    }
}

fn rename_new(directory: &Dir, from: &str, to: &str) -> Result<(), String> {
    let from = CString::new(from).map_err(|_| changed())?;
    let to = CString::new(to).map_err(|_| changed())?;
    // SAFETY: both paths are generated single components in the held directory.
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(changed())
    }
}

fn sync_directory(directory: &Dir) -> Result<(), String> {
    // SAFETY: the directory owns a live descriptor throughout this call.
    if unsafe { libc::fsync(directory.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(changed())
    }
}

async fn stop_for_rollback(config: &AppConfig, plan: &Plan) -> Result<(), String> {
    plan.revalidate(config, false).await?;
    let value = inspect(config).await?;
    if !matches!(
        value.pointer("/State/Status").and_then(|v| v.as_str()),
        Some("exited" | "created")
    ) {
        let mut command = Command::new(config.container_runtime.as_str());
        command.args(["stop", "--time", "10", &plan.container_id]);
        let output = process::output(
            command,
            CommandPolicy::new(Duration::from_secs(15), 4096),
            None,
            "UEFI recovery rollback stop",
        )
        .await
        .map_err(|_| changed())?;
        if !output.status.success() {
            return Err(changed());
        }
    }
    plan.revalidate(config, true).await
}

pub(crate) async fn recover(
    config: &AppConfig,
    id: &str,
    confirmed: bool,
) -> Result<Outcome, String> {
    if !confirmed {
        return Err(crate::tr!("error-nvram-confirmation-required"));
    }
    let _lease = super::maintenance::acquire(config, true)?;
    let mut plan = {
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if !pending.as_ref().is_some_and(|p| {
            p.preview.id == id && !p.replaced && p.created.elapsed() < PREVIEW_LIFETIME
        }) {
            return Err(changed());
        }
        pending.take().ok_or_else(changed)?
    };
    plan.revalidate(config, true).await?;
    let prepared = plan.backup_and_retire();
    if prepared.is_err() && !plan.replaced {
        return Err(changed());
    }
    let ready = if prepared.is_ok() {
        super::startup::ensure_guest_online_unlocked(config)
            .await
            .is_ok()
            && plan.revalidate(config, false).await.is_ok()
            && read_vars(&plan.directory, TARGET).is_ok_and(|(_, bytes)| !erased_store(&bytes))
    } else {
        false
    };
    if ready {
        return Ok(Outcome {
            ready: true,
            rolled_back: false,
            rollback_required: false,
        });
    }
    let rolled_back = stop_for_rollback(config, &plan).await.is_ok() && plan.restore().is_ok();
    if !rolled_back {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(plan);
    }
    Ok(Outcome {
        ready: false,
        rolled_back,
        rollback_required: !rolled_back,
    })
}

pub(crate) async fn restore(
    config: &AppConfig,
    id: &str,
    confirmed: bool,
) -> Result<Outcome, String> {
    if !confirmed {
        return Err(crate::tr!("error-nvram-confirmation-required"));
    }
    let _lease = super::maintenance::acquire(config, true)?;
    let mut plan = {
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if !pending
            .as_ref()
            .is_some_and(|p| p.preview.id == id && p.replaced)
        {
            return Err(changed());
        }
        pending.take().ok_or_else(changed)?
    };
    let result = async {
        plan.revalidate(config, true).await?;
        plan.restore()
    }
    .await;
    if result.is_err() {
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(plan);
        return Err(crate::tr!("error-nvram-rollback-required"));
    }
    Ok(Outcome {
        ready: false,
        rolled_back: true,
        rollback_required: false,
    })
}
