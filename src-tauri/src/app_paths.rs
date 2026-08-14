use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const APP_IDENTIFIER: &str = "com.ggobp.mendimaru";
const CONFIG_DIRECTORY_OVERRIDE: &str = "MENDIMARU_CONFIG_DIR";
const CACHE_DIRECTORY_OVERRIDE: &str = "MENDIMARU_CACHE_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppPaths {
    config_directory: PathBuf,
    cache_directory: PathBuf,
}

impl AppPaths {
    pub(crate) fn from_app(app: &AppHandle) -> Result<Self, String> {
        Ok(Self {
            config_directory: app.path().app_config_dir().map_err(|error| {
                format!("could not resolve the app configuration directory: {error}")
            })?,
            cache_directory: app
                .path()
                .app_cache_dir()
                .map_err(|error| format!("could not resolve the app cache directory: {error}"))?,
        })
    }

    pub(crate) fn discover_for_cli() -> Result<Self, String> {
        let (default_config, default_cache) = platform_directories()?;
        let config_directory = override_directory(CONFIG_DIRECTORY_OVERRIDE, default_config)?;
        let cache_directory = override_directory(CACHE_DIRECTORY_OVERRIDE, default_cache)?;
        Ok(Self {
            config_directory,
            cache_directory,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests(config_directory: PathBuf, cache_directory: PathBuf) -> Self {
        Self {
            config_directory,
            cache_directory,
        }
    }

    pub(crate) fn config_directory(&self) -> &Path {
        &self.config_directory
    }

    #[allow(dead_code)]
    pub(crate) fn cache_directory(&self) -> &Path {
        &self.cache_directory
    }

    pub(crate) fn ensure_config_directory(&self) -> Result<(), String> {
        ensure_direct_directory(&self.config_directory, "app configuration")
    }

    #[allow(dead_code)]
    pub(crate) fn ensure_cache_directory(&self) -> Result<(), String> {
        ensure_direct_directory(&self.cache_directory, "app cache")
    }
}

fn override_directory(name: &str, default: PathBuf) -> Result<PathBuf, String> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(default);
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{name} must be an absolute directory"));
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("{name} must reference a direct directory"));
        }
    }
    Ok(path)
}

fn ensure_direct_directory(path: &Path, label: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format!("could not create {label}: {error}"))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {label}: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("the {label} directory must be a direct directory"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_directories() -> Result<(PathBuf, PathBuf), String> {
    let home = crate::config::home_directory()?;
    let config_root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"));
    Ok((
        config_root.join(APP_IDENTIFIER),
        cache_root.join(APP_IDENTIFIER),
    ))
}

#[cfg(target_os = "windows")]
fn platform_directories() -> Result<(PathBuf, PathBuf), String> {
    let roaming = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "APPDATA is unavailable".to_string())?;
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| roaming.clone());
    Ok((roaming.join(APP_IDENTIFIER), local.join(APP_IDENTIFIER)))
}

#[cfg(target_os = "macos")]
fn platform_directories() -> Result<(PathBuf, PathBuf), String> {
    let home = crate::config::home_directory()?;
    Ok((
        home.join("Library/Application Support")
            .join(APP_IDENTIFIER),
        home.join("Library/Caches").join(APP_IDENTIFIER),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn platform_directories() -> Result<(PathBuf, PathBuf), String> {
    Err("the current host has no Mendimaru application directory contract".to_string())
}

#[cfg(test)]
mod tests {
    use super::{ensure_direct_directory, AppPaths};
    use std::fs;

    #[test]
    fn creates_only_direct_application_directories() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = temporary.path().join("config");
        let cache = temporary.path().join("cache");
        let paths = AppPaths::for_tests(config.clone(), cache.clone());
        paths.ensure_config_directory().expect("config directory");
        paths.ensure_cache_directory().expect("cache directory");
        assert!(config.is_dir());
        assert!(cache.is_dir());

        let file = temporary.path().join("not-a-directory");
        fs::write(&file, b"fixture").expect("fixture file");
        assert!(ensure_direct_directory(&file, "fixture").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_application_directory() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let target = temporary.path().join("target");
        let link = temporary.path().join("link");
        fs::create_dir(&target).expect("target directory");
        symlink(&target, &link).expect("directory symlink");
        assert!(ensure_direct_directory(&link, "fixture").is_err());
    }
}
