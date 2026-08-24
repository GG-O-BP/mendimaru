use crate::app_paths::AppPaths;
use crate::models::AppConfig;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use tauri::AppHandle;

const CONFIG_FILE_NAME: &str = "config.json";
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
pub(crate) struct ConfigSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

pub fn load_config(app: &AppHandle) -> Result<AppConfig, String> {
    load_config_from(&AppPaths::from_app(app)?)
}

pub(crate) fn load_config_from(paths: &AppPaths) -> Result<AppConfig, String> {
    let path = config_path(paths);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return super::detect_config()
        }
        Err(error) => return Err(crate::tr!("error-config-read", error = error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(crate::tr!(
            "error-config-read",
            error = "the configuration file must be a regular file"
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(crate::tr!(
            "error-config-read",
            error = "the configuration file exceeds the safe size limit"
        ));
    }
    let file =
        fs::File::open(&path).map_err(|error| crate::tr!("error-config-read", error = error))?;
    let mut content = String::with_capacity(metadata.len() as usize);
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|error| crate::tr!("error-config-read", error = error))?;
    let mut config = serde_json::from_str::<AppConfig>(&content)
        .map_err(|error| crate::tr!("error-config-parse", error = error))?;
    if super::migrate_legacy_windows_workspace(&mut config) {
        persist_config_from(paths, &config)?;
    }
    Ok(config)
}

pub fn persist_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    persist_config_from(&AppPaths::from_app(app)?, config)
}

pub(crate) fn persist_config_from(paths: &AppPaths, config: &AppConfig) -> Result<(), String> {
    paths.ensure_config_directory()?;
    let path = config_path(paths);
    let serialized = serde_json::to_vec_pretty(config)
        .map_err(|error| crate::tr!("error-config-serialize", error = error))?;
    atomic_write(&path, &serialized)
}

pub(crate) fn snapshot_config(app: &AppHandle) -> Result<ConfigSnapshot, String> {
    let path = config_path(&AppPaths::from_app(app)?);
    let content = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(crate::tr!(
                    "error-config-read",
                    error = "the configuration file must be a regular file"
                ));
            }
            if metadata.len() > MAX_CONFIG_BYTES {
                return Err(crate::tr!(
                    "error-config-read",
                    error = "the configuration file exceeds the safe size limit"
                ));
            }
            let mut content = Vec::with_capacity(metadata.len() as usize);
            fs::File::open(&path)
                .map_err(|error| crate::tr!("error-config-read", error = error))?
                .take(MAX_CONFIG_BYTES + 1)
                .read_to_end(&mut content)
                .map_err(|error| crate::tr!("error-config-read", error = error))?;
            Some(content)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(crate::tr!("error-config-read", error = error)),
    };
    Ok(ConfigSnapshot { path, content })
}

pub(crate) fn restore_config(snapshot: &ConfigSnapshot) -> Result<(), String> {
    match &snapshot.content {
        Some(content) => atomic_write(&snapshot.path, content),
        None if snapshot.path.exists() => fs::remove_file(&snapshot.path)
            .map_err(|error| crate::tr!("error-config-save", error = error)),
        None => Ok(()),
    }
}

fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<(), String> {
    let temporary_path = path.with_extension("json.tmp");
    let result = (|| {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(crate::tr!(
                    "error-config-save",
                    error = "the configuration file must be a regular file"
                ));
            }
        }
        if let Ok(metadata) = fs::symlink_metadata(&temporary_path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(crate::tr!(
                    "error-config-save",
                    error = "the temporary configuration file is unsafe"
                ));
            }
            fs::remove_file(&temporary_path)
                .map_err(|error| crate::tr!("error-config-save", error = error))?;
        }
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut temporary = options
            .open(&temporary_path)
            .map_err(|error| crate::tr!("error-config-save", error = error))?;
        use std::io::Write;
        temporary
            .write_all(content)
            .and_then(|_| temporary.sync_all())
            .map_err(|error| crate::tr!("error-config-save", error = error))?;
        replace_file(&temporary_path, path)
            .map_err(|error| crate::tr!("error-config-save", error = error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(target_os = "windows"))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
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

fn config_path(paths: &AppPaths) -> PathBuf {
    paths.config_directory().join(CONFIG_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::{atomic_write, load_config_from, persist_config_from};
    use crate::app_paths::AppPaths;
    use crate::models::AppConfig;
    use std::fs;

    #[test]
    fn atomic_write_replaces_the_target_without_leaving_a_temporary_file() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let target = temporary.path().join("config.json");
        fs::write(&target, b"old").expect("write original");

        atomic_write(&target, b"new").expect("replace config");

        assert_eq!(fs::read(&target).expect("read config"), b"new");
        assert!(!target.with_extension("json.tmp").exists());
    }

    #[test]
    fn headless_and_desktop_stores_share_the_same_explicit_path_contract() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        let expected = config_for_store(temporary.path());
        persist_config_from(&paths, &expected).expect("persist headless config");
        assert_eq!(
            load_config_from(&paths).expect("load headless config"),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_and_oversized_configuration_files() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        paths.ensure_config_directory().expect("config directory");
        let target = temporary.path().join("target.json");
        fs::write(&target, b"{}").expect("target config");
        symlink(&target, paths.config_directory().join("config.json")).expect("config symlink");
        assert!(load_config_from(&paths).is_err());

        fs::remove_file(paths.config_directory().join("config.json")).expect("remove symlink");
        fs::write(
            paths.config_directory().join("config.json"),
            vec![b'x'; super::MAX_CONFIG_BYTES as usize + 1],
        )
        .expect("oversized config");
        assert!(load_config_from(&paths).is_err());
    }

    fn config_for_store(workspace: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "compose.yml".into(),
            container_runtime: crate::models::ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: workspace.to_string_lossy().into_owned(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    #[test]
    fn linux_configuration_from_before_native_windows_support_migrates_safely() {
        let legacy = r#"{
          "languagePreference": "system",
          "winboatSetupPending": false,
          "winboatExecutable": "/usr/bin/winboat",
          "composeFile": "/home/dev/.winboat/docker-compose.yml",
          "containerRuntime": "docker",
          "containerName": "WinBoat",
          "apiUrl": "http://127.0.0.1:47280",
          "rdpHost": "127.0.0.1",
          "rdpPort": 47300,
          "sharedDirectory": "/home/dev/Mendix",
          "windowsSharedDirectory": "\\\\host.lan\\Data",
          "freerdpBinary": "xfreerdp3",
          "mendixInstallRoot": "C:\\Program Files\\Mendix",
          "mendixDataRoot": "C:\\ProgramData\\Mendix",
          "startupTimeoutSeconds": 180
        }"#;

        let migrated: AppConfig = serde_json::from_str(legacy).expect("legacy config");
        assert!(migrated.windows_studio_paths.is_empty());
        assert_eq!(migrated.winboat_executable, "/usr/bin/winboat");
        assert_eq!(migrated.shared_directory, "/home/dev/Mendix");
    }
}
