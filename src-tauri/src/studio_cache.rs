use crate::app_paths::AppPaths;
use crate::models::{AppConfig, InstalledVersionsCache, StudioVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

const CACHE_FILE_NAME: &str = "installed-studio-versions.json";
const CACHE_SCHEMA_VERSION: &str = "1.0.0";
const MAX_CACHE_BYTES: u64 = 512 * 1024;
const MAX_VERSIONS: usize = 256;
const MAX_FIELD_BYTES: usize = 8 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheRecord {
    schema_version: String,
    source_identity: String,
    captured_at: String,
    versions: Vec<StudioVersion>,
}

pub(crate) fn load(paths: &AppPaths, config: &AppConfig) -> Result<InstalledVersionsCache, String> {
    paths.ensure_cache_directory()?;
    let path = cache_path(paths);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstalledVersionsCache::default())
        }
        Err(error) => {
            return Err(format!(
                "could not inspect installed-version cache: {error}"
            ))
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("installed-version cache must be a regular file".to_string());
    }
    if metadata.len() > MAX_CACHE_BYTES {
        return Err("installed-version cache exceeds the safe size limit".to_string());
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(&path)
        .and_then(|file| file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut content))
        .map_err(|error| format!("could not read installed-version cache: {error}"))?;
    if content.len() as u64 > MAX_CACHE_BYTES {
        return Err("installed-version cache exceeds the safe size limit".to_string());
    }
    let record: CacheRecord = serde_json::from_slice(&content)
        .map_err(|error| format!("installed-version cache is invalid: {error}"))?;
    if record.schema_version != CACHE_SCHEMA_VERSION
        || record.source_identity != source_identity(config)
        || chrono::DateTime::parse_from_rfc3339(&record.captured_at).is_err()
    {
        return Err("installed-version cache does not match the active environment".to_string());
    }
    validate_versions(&record.versions)?;
    Ok(InstalledVersionsCache {
        versions: record.versions,
        captured_at: Some(record.captured_at),
    })
}

pub(crate) fn save(
    paths: &AppPaths,
    config: &AppConfig,
    versions: &[StudioVersion],
) -> Result<(), String> {
    validate_versions(versions)?;
    paths.ensure_cache_directory()?;
    let path = cache_path(paths);
    let record = CacheRecord {
        schema_version: CACHE_SCHEMA_VERSION.to_string(),
        source_identity: source_identity(config),
        captured_at: chrono::Utc::now().to_rfc3339(),
        versions: versions.to_vec(),
    };
    let content = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("could not serialize installed-version cache: {error}"))?;
    if content.len() as u64 > MAX_CACHE_BYTES {
        return Err("installed-version cache exceeds the safe size limit".to_string());
    }
    atomic_write(&path, &content)
}

fn source_identity(config: &AppConfig) -> String {
    let mut digest = Sha256::new();
    for value in [
        std::env::consts::OS,
        config.container_runtime.as_str(),
        &config.container_name,
        &config.api_url,
        &config.mendix_install_root,
        &config.mendix_data_root,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    for path in &config.windows_studio_paths {
        digest.update(path.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn validate_versions(versions: &[StudioVersion]) -> Result<(), String> {
    if versions.len() > MAX_VERSIONS {
        return Err("installed-version cache contains too many versions".to_string());
    }
    let mut seen = HashSet::new();
    for version in versions {
        crate::platform::validate_version(&version.version)
            .map_err(|_| "installed-version cache contains an invalid version".to_string())?;
        if !seen.insert(version.version.as_str()) {
            return Err("installed-version cache contains duplicate versions".to_string());
        }
        for field in [
            &version.display_name,
            &version.executable_path,
            &version.install_root,
            &version.source,
        ] {
            if field.len() > MAX_FIELD_BYTES || field.contains('\0') {
                return Err("installed-version cache contains an invalid field".to_string());
            }
        }
    }
    Ok(())
}

fn cache_path(paths: &AppPaths) -> std::path::PathBuf {
    paths.cache_directory().join(CACHE_FILE_NAME)
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let result = (|| {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("installed-version cache must be a regular file".to_string());
            }
        }
        if let Ok(metadata) = fs::symlink_metadata(&temporary) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("temporary installed-version cache is unsafe".to_string());
            }
            fs::remove_file(&temporary)
                .map_err(|error| format!("could not replace installed-version cache: {error}"))?;
        }
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("could not create installed-version cache: {error}"))?;
        file.write_all(content)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("could not write installed-version cache: {error}"))?;
        replace_file(&temporary, path)
            .map_err(|error| format!("could not finalize installed-version cache: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContainerRuntime;

    fn version(value: &str) -> StudioVersion {
        StudioVersion {
            version: value.to_string(),
            display_name: format!("Studio Pro {value}"),
            executable_path: format!(r"C:\Program Files\Mendix\{value}\modeler\StudioPro.exe"),
            install_root: format!(r"C:\Program Files\Mendix\{value}"),
            source: "fixture".to_string(),
            removable: true,
        }
    }

    fn config() -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "/tmp/compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: "/tmp/workspace".into(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    #[test]
    fn round_trips_a_private_bounded_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        let expected = vec![version("11.12.3"), version("10.24.24")];
        let config = config();
        save(&paths, &config, &expected).expect("save cache");
        let loaded = load(&paths, &config).expect("load cache");
        assert_eq!(loaded.versions, expected);
        assert!(loaded.captured_at.is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(cache_path(&paths))
                    .expect("cache metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_oversized_and_forged_caches() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        paths.ensure_cache_directory().expect("cache directory");
        let config = config();
        let cache = cache_path(&paths);
        let target = temporary.path().join("target.json");
        fs::write(&target, b"{}").expect("target");
        symlink(&target, &cache).expect("cache symlink");
        assert!(load(&paths, &config).is_err());

        fs::remove_file(&cache).expect("remove symlink");
        fs::write(&cache, vec![b'x'; MAX_CACHE_BYTES as usize + 1]).expect("oversized cache");
        assert!(load(&paths, &config).is_err());

        fs::write(
            &cache,
            br#"{"schemaVersion":"1.0.0","sourceIdentity":"bad","capturedAt":"2026-08-22T00:00:00Z","versions":[{"version":"../bad","displayName":"bad","executablePath":"bad","installRoot":"bad","source":"bad","removable":true}]}"#,
        )
        .expect("forged cache");
        assert!(load(&paths, &config).is_err());
    }

    #[test]
    fn rejects_a_cache_from_another_environment() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        let first = config();
        save(&paths, &first, &[version("11.12.3")]).expect("save cache");
        let mut second = first.clone();
        second.container_name = "OtherWinBoat".into();
        assert!(load(&paths, &second).is_err());
    }
}
