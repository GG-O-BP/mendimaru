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
pub(crate) const UNTRUSTED: &str = "the private Mendimaru coordination lock failed its ownership or permission check; review the environment diagnostics";
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
        error.code = BackendErrorCode::PreconditionFailed;
        error.retryable = self.retryable();
        error
    }

    pub(crate) fn command_error(self, capability: CapabilityId) -> CommandError {
        self.backend_error(capability).into()
    }

    /// Recovers the cause from a stable reason or its localized message.
    pub(crate) fn from_message(message: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|error| message == error.reason() || message == error.localized())
    }

    /// Environment defects only; transient contention is not one.
    pub(crate) const fn diagnostic_code(self) -> Option<EnvironmentDiagnosticErrorCode> {
        match self {
            Self::Busy | Self::Timeout => None,
            Self::Untrusted => Some(EnvironmentDiagnosticErrorCode::CoordinationLockUntrusted),
            Self::Unavailable => Some(EnvironmentDiagnosticErrorCode::CoordinationLockUnavailable),
            Self::Identity => Some(EnvironmentDiagnosticErrorCode::CoordinationIdentityMismatch),
        }
    }
}

impl From<LockError> for String {
    fn from(error: LockError) -> Self {
        error.localized()
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
        // An absent Compose file cannot be eligible for recovery.
        if !std::path::Path::new(&config.compose_file).exists() {
            return Ok(None);
        }
        acquire(config, false).map(Some)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Ok(None)
    }
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
        linux::open(config, linux::Kind::Maintenance)?;
        linux::open(config, linux::Kind::RuntimeStop)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Ok(())
    }
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
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(LockError::Untrusted);
        }
        Ok(directory)
    }

    /// A direct, regular, single-link, owner-only file. Never unlinked.
    pub(crate) fn open_lock(directory: &Dir, name: &str) -> Result<File, LockError> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        let file = directory
            .open_with(name, &options)
            .map_err(|error| classify(&error))?
            .into_std();
        let metadata = file.metadata().map_err(|_| LockError::Unavailable)?;
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
        {
            return Err(LockError::Untrusted);
        }
        Ok(file)
    }

    /// Older versions coordinate only through `<compose dir>/<legacy name>` and
    /// only when that directory and file pass the original trust checks. Join
    /// that lock exactly when an older version could hold it, so mixed versions
    /// can never overlap. Where older versions refuse the location (for example
    /// a group-writable `775` directory), they refuse every coordinated operation
    /// as well, so there is nothing to coordinate with. Unexpected I/O failures
    /// fail closed.
    pub(crate) fn legacy(compose: &Path, kind: Kind) -> Result<Option<File>, LockError> {
        let Some(parent) = compose.parent() else {
            return Ok(None);
        };
        let directory = match crate::winboat::nvram::open_directory(parent) {
            Ok(directory) => directory,
            Err(error) if refused_by_older_versions(&error) => return Ok(None),
            Err(_) => return Err(LockError::Unavailable),
        };
        match open_lock(&directory, kind.legacy_name()) {
            Ok(file) => Ok(Some(file)),
            Err(LockError::Untrusted) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn refused_by_older_versions(error: &std::io::Error) -> bool {
        matches!(
            error.kind(),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
        ) || classify(error) == LockError::Untrusted
    }

    /// Ownership, link and type refusals are trust failures; the rest is I/O.
    fn classify(error: &std::io::Error) -> LockError {
        if error.kind() == std::io::ErrorKind::PermissionDenied
            || matches!(
                error.raw_os_error(),
                Some(libc::ELOOP | libc::ENOTDIR | libc::EISDIR | libc::ENXIO | libc::EPERM)
            )
        {
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
