//! Same-user, host-wide advisory VM use. See docs/winboat-vm-use.md.
use crate::contracts::{BackendError, CapabilityId};
#[cfg(target_os = "linux")]
use crate::contracts::{BackendErrorCode, BackendId};
use crate::models::AppConfig;
use std::future::Future;
use std::time::Duration;

pub(crate) const BUSY: &str = "WinBoat VM is busy with another participant; retry after its use or lifecycle operation finishes";
pub(crate) const UNTRUSTED: &str =
    "WinBoat VM use lock could not be verified; check lock ownership and permissions";
pub(crate) const UPGRADE: &str =
    "WinBoat shared use cannot be upgraded; finish shared use before requesting a lifecycle change";
pub(crate) const WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Shared,
    Exclusive,
}

pub(crate) struct Lease {
    #[cfg(target_os = "linux")]
    inner: std::sync::Arc<linux::Owned>,
}

impl Lease {
    /// Only the current async call chain can reuse a lease. Spawned tasks and
    /// other processes acquire their own; a shared lease never grants mutation.
    pub(crate) async fn run<F: Future>(self, future: F) -> F::Output {
        #[cfg(target_os = "linux")]
        {
            let mut held = linux::HELD.try_with(Clone::clone).unwrap_or_default();
            held.push(self.inner);
            linux::HELD.scope(held, future).await
        }
        #[cfg(not(target_os = "linux"))]
        future.await
    }
}

pub(crate) async fn acquire(
    config: &AppConfig,
    mode: Mode,
    capability: CapabilityId,
) -> Result<Lease, BackendError> {
    acquire_for(config, mode, capability, WAIT).await
}

pub(crate) async fn acquire_for(
    config: &AppConfig,
    mode: Mode,
    capability: CapabilityId,
    wait: Duration,
) -> Result<Lease, BackendError> {
    #[cfg(target_os = "linux")]
    {
        linux::acquire(config, mode, wait.min(WAIT))
            .await
            .map_err(|message| error(capability, message))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, mode, capability, wait);
        Ok(Lease {})
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct Observation {
    key: String,
    file: std::fs::File,
}
#[cfg(target_os = "linux")]
impl Observation {
    pub(crate) fn snapshot(&self) -> Option<(String, String)> {
        use std::os::unix::fs::FileExt;
        let mut bytes = [0u8; 16];
        let length = self.file.read_at(&mut bytes, 0).ok()?;
        let generation = match length {
            0 => "uninitialized".into(),
            16 => bytes.iter().map(|b| format!("{b:02x}")).collect(),
            _ => return None,
        };
        Some((self.key.clone(), generation))
    }
}
#[cfg(target_os = "linux")]
pub(crate) fn observation() -> Option<Observation> {
    linux::observation()
}

#[cfg(target_os = "linux")]
fn error(capability: CapabilityId, message: &'static str) -> BackendError {
    let mut error = BackendError::operation(BackendId::LinuxWinboat, capability, message);
    error.code = BackendErrorCode::PreconditionFailed;
    error.retryable = message == BUSY;
    error
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt, PermissionsExt};
    use sha2::{Digest, Sha256};
    use std::fs::File;
    use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt};
    use std::sync::Arc;

    tokio::task_local! {
        pub(super) static HELD: Vec<Arc<Owned>>;
    }

    pub(super) struct Owned {
        // Retained until the final scoped owner drops; never explicitly unlock.
        _file: File,
        key: String,
        mode: Mode,
        owner: ProcessIdentity,
    }

    // Never persisted, never used to kill a process or remove a lock. flock's
    // open-file ownership is authoritative even if a PID or stale file is reused.
    #[derive(Debug, PartialEq, Eq)]
    struct ProcessIdentity {
        pid: u32,
        start: u64,
    }

    fn process_identity() -> Result<ProcessIdentity, &'static str> {
        let pid = std::process::id();
        let stat = std::fs::read_to_string("/proc/self/stat").map_err(|_| UNTRUSTED)?;
        let (_, fields) = stat.rsplit_once(") ").ok_or(UNTRUSTED)?;
        let start = fields
            .split_whitespace()
            .nth(19)
            .and_then(|v| v.parse().ok())
            .filter(|v| *v > 0)
            .ok_or(UNTRUSTED)?;
        Ok(ProcessIdentity { pid, start })
    }

    fn key(config: &AppConfig) -> Result<String, &'static str> {
        let name = &config.container_name;
        if name.is_empty()
            || name.len() > 255
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(UNTRUSTED);
        }
        let compose = std::path::Path::new(&config.compose_file);
        match std::fs::symlink_metadata(compose) {
            Ok(_) => {
                if crate::config::winboat_management_name(compose).map_err(|_| UNTRUSTED)? != *name
                {
                    return Err(UNTRUSTED);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(UNTRUSTED),
        }
        // Deliberately independent of cache, Compose path, Docker context and
        // endpoint aliases. Same-named VMs on different daemons over-exclude.
        // Container IDs are generations, never management identities.
        Ok(format!(
            "{:x}",
            Sha256::digest(format!(
                "vm-use-v1\0{}\0{name}",
                config.container_runtime.as_str()
            ))
        ))
    }

    fn directory() -> Result<Dir, &'static str> {
        let uid = unsafe { libc::geteuid() };
        // A fixed local namespace: TMPDIR/XDG/cache overrides cannot split it.
        let path = std::path::PathBuf::from(format!("/tmp/mendimaru-vm-use-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(UNTRUSTED),
        }
        trusted_directory(&path)
    }

    fn trusted_directory(path: &std::path::Path) -> Result<Dir, &'static str> {
        let directory = super::super::nvram::open_directory(path).map_err(|_| UNTRUSTED)?;
        let metadata = directory.dir_metadata().map_err(|_| UNTRUSTED)?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(UNTRUSTED);
        }
        Ok(directory)
    }

    fn open(directory: &Dir, key: &str) -> Result<File, &'static str> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = directory
            .open_with(format!("{key}.lock"), &options)
            .map_err(|_| UNTRUSTED)?
            .into_std();
        let metadata = file.metadata().map_err(|_| UNTRUSTED)?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
        {
            return Err(UNTRUSTED);
        }
        Ok(file)
    }

    pub(super) fn observation() -> Option<Observation> {
        HELD.try_with(|leases| {
            let held = leases.last()?;
            if held.owner != process_identity().ok()? {
                return None;
            }
            Some(Observation {
                key: held.key.clone(),
                file: held._file.try_clone().ok()?,
            })
        })
        .ok()
        .flatten()
    }

    pub(super) async fn acquire(
        config: &AppConfig,
        mode: Mode,
        wait: Duration,
    ) -> Result<Lease, &'static str> {
        let key = key(config)?;
        let owner = process_identity()?;
        if let Some(held) = HELD
            .try_with(|leases| leases.iter().find(|lease| lease.key == key).cloned())
            .ok()
            .flatten()
        {
            if held.owner != owner {
                return Err(UNTRUSTED);
            }
            if held.mode == Mode::Shared && mode == Mode::Exclusive {
                return Err(UPGRADE);
            }
            return Ok(Lease { inner: held });
        }
        let file = open(&directory()?, &key)?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let result = match mode {
                Mode::Shared => fs2::FileExt::try_lock_shared(&file),
                Mode::Exclusive => fs2::FileExt::try_lock_exclusive(&file),
            };
            match result {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(UNTRUSTED),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(BUSY);
            }
            tokio::time::sleep_until(
                deadline.min(tokio::time::Instant::now() + Duration::from_millis(25)),
            )
            .await;
        }
        // A settings transaction may have changed the Compose target while we
        // waited. Revalidate before allowing this caller to act on that file.
        if self::key(config)? != key {
            return Err(UNTRUSTED);
        }
        // A fixed-size opaque generation token changes before each exclusive
        // transaction, including recovery. It is diagnostic, never lock ownership.
        // An interrupted write cannot make a live lease stale or permit eviction.
        if mode == Mode::Exclusive {
            let mut generation = [0u8; 16];
            getrandom::fill(&mut generation).map_err(|_| UNTRUSTED)?;
            file.write_all_at(&generation, 0).map_err(|_| UNTRUSTED)?;
            file.set_len(16).map_err(|_| UNTRUSTED)?;
        }
        Ok(Lease {
            inner: Arc::new(Owned {
                _file: file,
                key,
                mode,
                owner,
            }),
        })
    }

    #[cfg(test)]
    mod tests;
}
