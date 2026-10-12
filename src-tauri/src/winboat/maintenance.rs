//! Cross-process coordination between UEFI maintenance, Runtime stops and
//! ordinary Mendimaru operations. See docs/winboat-coordination-locks.md.
//!
//! The locks live in a fixed Mendimaru-owned private namespace keyed by the VM
//! management identity, never in the WinBoat-managed Compose directory whose
//! permissions belong to another application (#225).
use crate::contracts::{BackendError, BackendErrorCode, BackendId, CapabilityId};
use crate::models::{AppConfig, CommandError, EnvironmentDiagnosticErrorCode};

// Exact, path-free reasons. The CLI only lets these survive redaction.
pub(crate) const BUSY: &str =
    "another Mendimaru operation or UEFI recovery is using this WinBoat VM; retry after it finishes";
pub(crate) const UNTRUSTED: &str = "a Mendimaru coordination entry failed its ownership, direct-path or permission check; review the environment diagnostics";
pub(crate) const UNAVAILABLE: &str =
    "the private Mendimaru coordination lock could not be opened or locked";
pub(crate) const IDENTITY: &str = "the WinBoat Compose file does not identify the configured VM; check the Compose path and container name";
pub(crate) const TIMEOUT: &str =
    "another Runtime stop for this WinBoat VM did not finish before the deadline; retry after it finishes";

/// Why a coordination lock could not be held. Each cause stays distinct up to
/// the UI and CLI: a permission problem is never reported as a busy lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockError {
    /// Another Mendimaru operation (current or older version) holds the lock.
    Busy,
    /// The private directory or lock file failed ownership/mode/type checks.
    Untrusted,
    /// Any other I/O failure while creating, opening or locking.
    Unavailable,
    /// The Compose file names a different VM than the configuration.
    Identity,
    /// A bounded wait for the lock expired.
    Timeout,
}

impl LockError {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] = [
        Self::Busy,
        Self::Untrusted,
        Self::Unavailable,
        Self::Identity,
        Self::Timeout,
    ];

    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::Busy => BUSY,
            Self::Untrusted => UNTRUSTED,
            Self::Unavailable => UNAVAILABLE,
            Self::Identity => IDENTITY,
            Self::Timeout => TIMEOUT,
        }
    }

    /// User-facing message with the actual cause and a safe next step.
    pub(crate) fn localized(self) -> String {
        match self {
            Self::Busy => crate::tr!("error-coordination-busy"),
            Self::Untrusted => crate::tr!("error-coordination-untrusted"),
            Self::Unavailable => crate::tr!("error-coordination-unavailable"),
            Self::Identity => crate::tr!("error-coordination-identity"),
            Self::Timeout => crate::tr!("error-coordination-timeout"),
        }
    }

    /// Only contention resolves by itself; trust and I/O problems do not.
    pub(crate) const fn retryable(self) -> bool {
        matches!(self, Self::Busy | Self::Timeout)
    }

    pub(crate) fn backend_error(self, capability: CapabilityId) -> BackendError {
        let mut error =
            BackendError::operation(BackendId::LinuxWinboat, capability, self.localized());
        error.code = self.code();
        error.retryable = self.retryable();
        error
    }

    pub(crate) const fn code(self) -> BackendErrorCode {
        match self {
            Self::Busy => BackendErrorCode::CoordinationBusy,
            Self::Untrusted => BackendErrorCode::CoordinationUntrusted,
            Self::Unavailable => BackendErrorCode::CoordinationUnavailable,
            Self::Identity => BackendErrorCode::CoordinationIdentityMismatch,
            Self::Timeout => BackendErrorCode::CoordinationTimeout,
        }
    }

    pub(crate) const fn from_code(code: BackendErrorCode) -> Option<Self> {
        match code {
            BackendErrorCode::CoordinationBusy => Some(Self::Busy),
            BackendErrorCode::CoordinationUntrusted => Some(Self::Untrusted),
            BackendErrorCode::CoordinationUnavailable => Some(Self::Unavailable),
            BackendErrorCode::CoordinationIdentityMismatch => Some(Self::Identity),
            BackendErrorCode::CoordinationTimeout => Some(Self::Timeout),
            _ => None,
        }
    }

    pub(crate) fn command_error(self, capability: CapabilityId) -> CommandError {
        self.backend_error(capability).into()
    }

    /// Display-only compatibility helper. Operational decisions use from_code,
    /// never the currently selected locale or human-readable message.
    #[cfg(test)]
    pub(crate) fn from_message(message: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|error| message == error.reason() || message == error.localized())
    }

    /// Environment defects only; transient contention is not one.
    pub(crate) const fn diagnostic_code(self) -> Option<EnvironmentDiagnosticErrorCode> {
        match self {
            Self::Busy | Self::Timeout => {
                Some(EnvironmentDiagnosticErrorCode::CoordinationLockBusy)
            }
            Self::Untrusted => Some(EnvironmentDiagnosticErrorCode::CoordinationLockUntrusted),
            Self::Unavailable => Some(EnvironmentDiagnosticErrorCode::CoordinationLockUnavailable),
            Self::Identity => Some(EnvironmentDiagnosticErrorCode::CoordinationIdentityMismatch),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Lease {
    // Closing every descriptor releases the advisory locks; never unlink them.
    #[cfg(target_os = "linux")]
    _files: Vec<std::fs::File>,
}

/// Shared maintenance use for ordinary operations. Immediate, never waits.
pub(crate) fn shared(config: &AppConfig) -> Result<Option<Lease>, LockError> {
    #[cfg(target_os = "linux")]
    {
        // Reserve the management identity even before Compose exists. exists()
        // also hides permission errors and dangling symlinks, so is not a trust
        // check and must never authorize skipping coordination.
        acquire(config, false).map(Some)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn validate_identity(config: &AppConfig) -> Result<(), LockError> {
    linux::validate_compose_path(std::path::Path::new(&config.compose_file))?;
    crate::winboat::vm_use::identity_key(config).map_err(|_| LockError::Identity)?;
    Ok(())
}

/// Exclusive (UEFI recovery) or shared maintenance use. Immediate, never waits.
#[cfg(target_os = "linux")]
pub(super) fn acquire(config: &AppConfig, exclusive: bool) -> Result<Lease, LockError> {
    let files = linux::open(config, linux::Kind::Maintenance)?;
    for file in &files {
        linux::try_lock(file, exclusive)?;
    }
    Ok(Lease { _files: files })
}

/// Serialize Runtime stops for one VM, waiting at most `timeout`.
#[cfg(target_os = "linux")]
pub(crate) async fn runtime_stop(
    config: &AppConfig,
    timeout: std::time::Duration,
) -> Result<Lease, LockError> {
    let files = linux::open(config, linux::Kind::RuntimeStop)?;
    linux::wait_exclusive(files, timeout)
        .await
        .map(|files| Lease { _files: files })
}

/// Read-only preflight for environment diagnostics: verifies that every lock
/// an operation would need can be opened and trusted, without holding any.
pub(crate) fn probe(config: &AppConfig) -> Result<(), LockError> {
    #[cfg(target_os = "linux")]
    {
        let files = linux::open(config, linux::Kind::Maintenance)?;
        for file in &files {
            linux::try_lock(file, false)?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Ok(())
    }
}

/// A stop-only defect must not disable unrelated Studio/installation actions.
pub(crate) fn probe_runtime_stop(config: &AppConfig) -> Result<(), LockError> {
    #[cfg(target_os = "linux")]
    {
        let files = linux::open(config, linux::Kind::RuntimeStop)?;
        for file in &files {
            linux::try_lock(file, true)?;
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = config;
    Ok(())
}

/// The private lock file a configuration resolves to, for fixtures only.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn private_lock_path(config: &AppConfig, runtime_stop: bool) -> std::path::PathBuf {
    let key = crate::winboat::vm_use::identity_key(config).expect("fixture VM identity");
    let kind = if runtime_stop {
        "runtime-stop"
    } else {
        "maintenance"
    };
    linux::root().join(format!("{key}.{kind}.lock"))
}

#[cfg(target_os = "linux")]
pub(super) mod linux {
    use super::LockError;
    use crate::models::AppConfig;
    use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt, PermissionsExt};
    use std::fs::File;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Kind {
        Maintenance,
        RuntimeStop,
    }

    impl Kind {
        fn suffix(self) -> &'static str {
            match self {
                Self::Maintenance => "maintenance",
                Self::RuntimeStop => "runtime-stop",
            }
        }

        /// The name older Mendimaru versions use inside the Compose directory.
        pub(crate) fn legacy_name(self) -> &'static str {
            match self {
                Self::Maintenance => ".mendimaru-maintenance.lock",
                Self::RuntimeStop => ".mendimaru-runtime-stop.lock",
            }
        }
    }

    /// A fixed local namespace: TMPDIR/XDG/cache overrides cannot split it.
    pub(crate) fn root() -> PathBuf {
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        PathBuf::from(format!("/tmp/mendimaru-vm-coordination-{uid}"))
    }

    /// Every descriptor that must be locked, private lock first.
    pub(crate) fn open(config: &AppConfig, kind: Kind) -> Result<Vec<File>, LockError> {
        validate_compose_path(Path::new(&config.compose_file))?;
        let key = crate::winboat::vm_use::identity_key(config).map_err(|_| LockError::Identity)?;
        let mut files = vec![open_private(&root(), &key, kind)?];
        files.extend(legacy(Path::new(&config.compose_file), kind)?);
        Ok(files)
    }

    pub(crate) fn open_private(root: &Path, key: &str, kind: Kind) -> Result<File, LockError> {
        let directory = private_directory(root)?;
        open_lock(&directory, &format!("{key}.{}.lock", kind.suffix()))
    }

    fn private_directory(root: &Path) -> Result<Dir, LockError> {
        match std::fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(classify(&error)),
        }
        let directory =
            crate::winboat::nvram::open_directory(root).map_err(|error| classify(&error))?;
        let metadata = directory
            .dir_metadata()
            .map_err(|_| LockError::Unavailable)?;
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            return Err(LockError::Untrusted);
        }
        Ok(directory)
    }

    /// A direct, regular, single-link, owner-only file. Never unlinked.
    pub(crate) fn open_lock(directory: &Dir, name: &str) -> Result<File, LockError> {
        open_lock_with(directory, name, true)
    }

    fn open_lock_with(directory: &Dir, name: &str, create: bool) -> Result<File, LockError> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = if create {
            options.create_new(true);
            match directory.open_with(name, &options) {
                Ok(file) => {
                    // Only normalize a descriptor we just created. An unusual
                    // umask must not leave a fresh file unusable; never chmod a
                    // pre-existing file, path, link or another owner's entry.
                    let file = file.into_std();
                    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))
                        .map_err(|error| classify(&error))?;
                    file
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    options.create_new(false);
                    directory
                        .open_with(name, &options)
                        .map_err(|error| classify(&error))?
                        .into_std()
                }
                Err(error) => return Err(classify(&error)),
            }
        } else {
            directory
                .open_with(name, &options)
                .map_err(|error| classify(&error))?
                .into_std()
        };
        let metadata = file.metadata().map_err(|_| LockError::Unavailable)?;
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(LockError::Untrusted);
        }
        Ok(file)
    }

    /// Retain every existing legacy inode, including after directory mode
    /// changes: an older live holder does not lose its flock when chmod runs.
    /// Only an absent legacy file in a group-writable directory may be skipped.
    /// Unsafe existing entries fail closed; they are never repaired/unlinked.
    pub(crate) fn legacy(compose: &Path, kind: Kind) -> Result<Option<File>, LockError> {
        let Some(parent) = compose.parent().filter(|p| !p.as_os_str().is_empty()) else {
            return Ok(None);
        };
        let directory = match compose_directory(parent) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(classify(&error)),
        };
        match directory.symlink_metadata(kind.legacy_name()) {
            Ok(_) => open_lock_with(&directory, kind.legacy_name(), false).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let metadata = directory
                    .dir_metadata()
                    .map_err(|_| LockError::Unavailable)?;
                if metadata.permissions().mode() & 0o022 != 0 {
                    Ok(None)
                } else {
                    open_lock(&directory, kind.legacy_name()).map(Some)
                }
            }
            Err(error) => Err(classify(&error)),
        }
    }

    /// This is deliberately separate from NVRAM's stricter mutation policy.
    /// Normal user-owned WinBoat Compose directories may be group-writable,
    /// but not world-writable, foreign-owned or reached through links.
    fn compose_directory(path: &Path) -> std::io::Result<Dir> {
        let directory = owned_directory(path)?;
        if directory.dir_metadata()?.permissions().mode() & 0o002 != 0 {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        Ok(directory)
    }

    /// Direct owner-verified handles for the explicitly confirmed repair of
    /// Mendimaru's own namespace. Callers must never use this to mutate VM data.
    pub(super) fn owned_directory(path: &Path) -> std::io::Result<Dir> {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        use std::path::Component;
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
            let name = CString::new(name.as_bytes())?;
            // SAFETY: valid owned directory descriptor and C string; transfer
            // the successful descriptor into a File exactly once.
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
        if depth < 2 || metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o002 != 0
        {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        Ok(Dir::from_std_file(directory))
    }

    pub(super) fn validate_compose_path(compose: &Path) -> Result<(), LockError> {
        match std::fs::symlink_metadata(compose) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // symlink_metadata can report NotFound for a dangling ancestor.
                // Verify existing ancestors, rather than following their links.
                let mut parent = compose.parent();
                while let Some(path) = parent {
                    if path.as_os_str().is_empty() {
                        break;
                    }
                    match compose_directory(path) {
                        Ok(_) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            parent = path.parent();
                        }
                        Err(error) => return Err(classify(&error)),
                    }
                }
                Ok(())
            }
            Err(error) => Err(classify(&error)),
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.nlink() != 1
                    || metadata.mode() & 0o002 != 0
                {
                    return Err(LockError::Untrusted);
                }
                compose_directory(compose.parent().ok_or(LockError::Untrusted)?)
                    .map_err(|error| classify(&error))?;
                Ok(())
            }
        }
    }

    /// Ownership, link and type refusals are trust failures; the rest is I/O.
    pub(super) fn classify(error: &std::io::Error) -> LockError {
        if matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::InvalidInput
        ) || matches!(
            error.raw_os_error(),
            Some(libc::ELOOP | libc::ENOTDIR | libc::EISDIR | libc::ENXIO | libc::EPERM)
        ) {
            LockError::Untrusted
        } else {
            LockError::Unavailable
        }
    }

    pub(crate) fn try_lock(file: &File, exclusive: bool) -> Result<(), LockError> {
        let result = if exclusive {
            fs2::FileExt::try_lock_exclusive(file)
        } else {
            fs2::FileExt::try_lock_shared(file)
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Err(LockError::Busy),
            Err(_) => Err(LockError::Unavailable),
        }
    }

    /// Lock every file exclusively in order. Async polling keeps CLI deadlines
    /// and cancellation effective; dropping the future closes the descriptors.
    pub(crate) async fn wait_exclusive(
        files: Vec<File>,
        timeout: Duration,
    ) -> Result<Vec<File>, LockError> {
        let deadline = tokio::time::Instant::now() + timeout;
        for file in &files {
            loop {
                match try_lock(file, true) {
                    Ok(()) => break,
                    Err(LockError::Busy) => {}
                    Err(error) => return Err(error),
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(LockError::Timeout);
                }
                tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + POLL_INTERVAL))
                    .await;
            }
        }
        Ok(files)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(target_os = "linux")]
pub(crate) mod repair;
