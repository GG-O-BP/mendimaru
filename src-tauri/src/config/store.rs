use crate::app_paths::AppPaths;
use crate::models::AppConfig;
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;

const CONFIG_FILE_NAME: &str = "config.json";

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
    if !path.is_file() {
        return super::detect_config();
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| crate::tr!("error-config-read", error = error))?;
    serde_json::from_str::<AppConfig>(&content)
        .map_err(|error| crate::tr!("error-config-parse", error = error))
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
    let content = if path.is_file() {
        Some(fs::read(&path).map_err(|error| crate::tr!("error-config-read", error = error))?)
    } else {
        None
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
    fs::write(&temporary_path, content)
        .map_err(|error| crate::tr!("error-config-save", error = error))?;
    fs::rename(&temporary_path, path)
        .map_err(|error| crate::tr!("error-config-save", error = error))
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
