//! Cross-process exclusion between UEFI maintenance and Mendimaru operations.
use crate::models::AppConfig;

#[derive(Debug)]
pub(crate) struct Lease {
    #[cfg(target_os = "linux")]
    _file: std::fs::File,
}

pub(crate) fn shared(config: &AppConfig) -> Result<Option<Lease>, String> {
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

#[cfg(target_os = "linux")]
pub(super) fn acquire(config: &AppConfig, exclusive: bool) -> Result<Lease, String> {
    use std::os::unix::fs::MetadataExt;
    let parent = std::path::Path::new(&config.compose_file)
        .parent()
        .ok_or_else(rejected)?;
    let directory = super::nvram::open_directory(parent).map_err(|_| rejected())?;
    let mut options = cap_std::fs::OpenOptions::new();
    // cap-std confines path resolution; the final lock must also be a direct file.
    use cap_std::fs::OpenOptionsExt as _;
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = directory
        .open_with(".mendimaru-maintenance.lock", &options)
        .map_err(|_| rejected())?
        .into_std();
    let metadata = file.metadata().map_err(|_| rejected())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(rejected());
    }
    let result = if exclusive {
        fs2::FileExt::try_lock_exclusive(&file)
    } else {
        fs2::FileExt::try_lock_shared(&file)
    };
    result.map_err(|_| rejected())?;
    Ok(Lease { _file: file })
}

#[cfg(target_os = "linux")]
fn rejected() -> String {
    crate::tr!("error-nvram-operation-busy")
}
