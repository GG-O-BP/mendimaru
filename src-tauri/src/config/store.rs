use crate::models::AppConfig;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const CONFIG_FILE_NAME: &str = "config.json";

#[derive(Debug)]
pub(crate) struct ConfigSnapshot {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

pub fn load_config(app: &AppHandle) -> Result<AppConfig, String> {
    let path = config_path(app)?;
    if !path.is_file() {
        return super::detect_config();
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| crate::tr!("error-config-read", error = error))?;
    serde_json::from_str::<AppConfig>(&content)
        .map_err(|error| crate::tr!("error-config-parse", error = error))
}

pub fn persist_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| crate::tr!("error-config-directory-create", error = error))?;
    }
    let serialized = serde_json::to_vec_pretty(config)
        .map_err(|error| crate::tr!("error-config-serialize", error = error))?;
    atomic_write(&path, &serialized)
}

pub(crate) fn snapshot_config(app: &AppHandle) -> Result<ConfigSnapshot, String> {
    let path = config_path(app)?;
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

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(CONFIG_FILE_NAME))
        .map_err(|error| crate::tr!("error-app-config-path", error = error))
}

#[cfg(test)]
mod tests {
    use super::atomic_write;
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
}
