use crate::models::{AppConfig, MendixProject};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

const STORE_FILE_NAME: &str = "project-launches.json";
const STORE_SCHEMA_VERSION: &str = "1.0.0";
const MAX_STORE_BYTES: u64 = 128 * 1024;
const MAX_RECORDS: usize = 256;

static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectLaunchStore {
    schema_version: String,
    records: Vec<ProjectLaunchRecord>,
}

impl Default for ProjectLaunchStore {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION.to_string(),
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectLaunchRecord {
    project_key: String,
    selected_version: Option<String>,
    pending: bool,
    updated_at: DateTime<Utc>,
}

pub(crate) fn apply_preferences(
    app: &AppHandle,
    config: &AppConfig,
    projects: &mut [MendixProject],
) -> Result<(), String> {
    let path = store_path(app)?;
    apply_preferences_at(&path, config, projects)
}

pub(crate) fn remember(
    app: &AppHandle,
    config: &AppConfig,
    project_mpr_path: &str,
    selected_version: Option<&str>,
    pending: bool,
) -> Result<(), String> {
    remember_at(
        &store_path(app)?,
        config,
        project_mpr_path,
        selected_version,
        pending,
    )
}

fn apply_preferences_at(
    path: &Path,
    config: &AppConfig,
    projects: &mut [MendixProject],
) -> Result<(), String> {
    let _guard = lock_store()?;
    let store = load_store(path)?;
    for project in projects {
        let key = project_key(config, &project.mpr_path)?;
        if let Some(record) = store
            .records
            .iter()
            .find(|record| record.project_key == key)
        {
            project.preferred_version = record.selected_version.clone();
            project.launch_pending = record.pending;
        }
    }
    Ok(())
}

fn remember_at(
    path: &Path,
    config: &AppConfig,
    project_mpr_path: &str,
    selected_version: Option<&str>,
    pending: bool,
) -> Result<(), String> {
    let selected_version = selected_version
        .map(str::trim)
        .filter(|version| !version.is_empty());
    if let Some(version) = selected_version {
        crate::platform::validate_version(version)?;
    }
    let key = project_key(config, project_mpr_path)?;
    let _guard = lock_store()?;
    let mut store = load_store(path)?;
    store.records.retain(|record| record.project_key != key);
    store.records.push(ProjectLaunchRecord {
        project_key: key,
        selected_version: selected_version.map(ToString::to_string),
        pending,
        updated_at: Utc::now(),
    });
    store
        .records
        .sort_by_key(|record| std::cmp::Reverse(record.updated_at));
    store.records.truncate(MAX_RECORDS);
    save_store(path, &store)
}

fn project_key(config: &AppConfig, project_mpr_path: &str) -> Result<String, String> {
    let selection =
        crate::projects::validate_project_selection(config, Path::new(project_mpr_path))?;
    Ok(selection.project_digest())
}

fn load_store(path: &Path) -> Result<ProjectLaunchStore, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectLaunchStore::default());
        }
        Err(error) => {
            return Err(format!(
                "could not inspect project launch preferences: {error}"
            ))
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("project launch preferences must be a regular file".into());
    }
    if metadata.len() > MAX_STORE_BYTES {
        return Err("project launch preferences exceed the safe size limit".into());
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(MAX_STORE_BYTES + 1).read_to_end(&mut content))
        .map_err(|error| format!("could not read project launch preferences: {error}"))?;
    if content.len() as u64 > MAX_STORE_BYTES {
        return Err("project launch preferences exceed the safe size limit".into());
    }
    let store = serde_json::from_slice::<ProjectLaunchStore>(&content)
        .map_err(|error| format!("project launch preferences are invalid: {error}"))?;
    validate_store(&store)?;
    Ok(store)
}

fn validate_store(store: &ProjectLaunchStore) -> Result<(), String> {
    if store.schema_version != STORE_SCHEMA_VERSION || store.records.len() > MAX_RECORDS {
        return Err("project launch preferences use an unsupported shape".into());
    }
    let mut keys = HashSet::new();
    for record in &store.records {
        if record.project_key.len() != 64
            || !record
                .project_key
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !keys.insert(&record.project_key)
        {
            return Err("project launch preferences contain an invalid project key".into());
        }
        if let Some(version) = &record.selected_version {
            crate::platform::validate_version(version)?;
        }
    }
    Ok(())
}

fn save_store(path: &Path, store: &ProjectLaunchStore) -> Result<(), String> {
    validate_store(store)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("project launch preferences must be a regular file".into());
        }
    }
    let content = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("could not serialize project launch preferences: {error}"))?;
    if content.len() as u64 > MAX_STORE_BYTES {
        return Err("project launch preferences exceed the safe size limit".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "project launch preferences have no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create project launch preferences: {error}"))?;
    let temporary = temporary_path(path)?;
    let result =
        write_private_file(&temporary, &content).and_then(|()| replace_file(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    sync_parent(path);
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "project launch preferences have no parent directory".to_string())?;
    for _ in 0..8 {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random)
            .map_err(|error| format!("could not create a preference transaction: {error}"))?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let candidate = parent.join(format!(".{STORE_FILE_NAME}.{suffix}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("could not allocate a project preference transaction".into())
}

fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not create project launch preferences: {error}"))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not save project launch preferences: {error}"))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("could not replace project launch preferences: {error}"))
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(format!(
            "could not replace project launch preferences: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) {}

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(STORE_FILE_NAME))
        .map_err(|error| format!("could not locate project launch preferences: {error}"))
}

fn lock_store() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "project launch preference lock is poisoned".into())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_preferences_at, load_store, remember_at, save_store, ProjectLaunchRecord,
        ProjectLaunchStore, MAX_RECORDS, STORE_SCHEMA_VERSION,
    };
    use crate::models::{AppConfig, ContainerRuntime, MendixProject};
    use chrono::{Duration, Utc};
    use std::fs;

    fn config(path: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: "compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: path.to_string_lossy().to_string(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn project(path: &std::path::Path) -> MendixProject {
        MendixProject {
            name: "Orders".into(),
            directory: path
                .parent()
                .expect("project directory")
                .to_string_lossy()
                .to_string(),
            mpr_path: path.to_string_lossy().to_string(),
            windows_path: r"\\host.lan\Data\Orders\Orders.mpr".into(),
            location: crate::models::ProjectLocation::ConfiguredWorkspace,
            version: None,
            preferred_version: None,
            launch_pending: false,
            last_modified: None,
        }
    }

    #[test]
    fn remembers_only_a_hashed_project_identity_and_restores_pending_state() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let project_directory = temporary.path().join("Orders");
        fs::create_dir(&project_directory).expect("project directory");
        let mpr = project_directory.join("Orders.mpr");
        fs::write(&mpr, b"fixture").expect("project fixture");
        let store_path = temporary.path().join("config/project-launches.json");
        let config = config(temporary.path());

        remember_at(
            &store_path,
            &config,
            &mpr.to_string_lossy(),
            Some("11.12.2"),
            true,
        )
        .expect("remember preference");
        let serialized = fs::read_to_string(&store_path).expect("preference file");
        assert!(!serialized.contains("Orders"));
        assert!(!serialized.contains(&mpr.to_string_lossy().to_string()));

        let mut projects = vec![project(&mpr)];
        apply_preferences_at(&store_path, &config, &mut projects).expect("apply preference");
        assert_eq!(projects[0].preferred_version.as_deref(), Some("11.12.2"));
        assert!(projects[0].launch_pending);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_hashed_outside_projects_but_rejects_invalid_versions_and_forged_stores() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let outside_directory = outside.path().join("Outside");
        fs::create_dir(&outside_directory).expect("outside directory");
        let outside_project = outside_directory.join("Outside.mpr");
        fs::write(&outside_project, b"fixture").expect("outside fixture");
        let store_path = workspace.path().join("project-launches.json");
        let config = config(workspace.path());

        remember_at(
            &store_path,
            &config,
            &outside_project.to_string_lossy(),
            Some("11.12.2"),
            true,
        )
        .expect("outside preference");
        let serialized = fs::read_to_string(&store_path).expect("preference store");
        assert!(!serialized.contains("Outside"));
        assert!(!serialized.contains(&outside_project.to_string_lossy().to_string()));

        let inside = workspace.path().join("Inside.mpr");
        fs::write(&inside, b"fixture").expect("inside fixture");
        assert!(remember_at(
            &store_path,
            &config,
            &inside.to_string_lossy(),
            Some("../11.12.2"),
            true,
        )
        .is_err());

        fs::write(
            &store_path,
            br#"{"schemaVersion":"1.0.0","records":[],"forged":true}"#,
        )
        .expect("forged store");
        assert!(load_store(&store_path).is_err());

        fs::write(&store_path, vec![b'x'; 128 * 1024 + 1]).expect("oversized store");
        assert!(load_store(&store_path).is_err());
    }

    #[test]
    fn bounds_the_number_of_remembered_projects() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let store_path = temporary.path().join("config/project-launches.json");
        let config = config(temporary.path());
        let now = Utc::now();
        let records = (0..MAX_RECORDS)
            .map(|index| ProjectLaunchRecord {
                project_key: format!("{index:064x}"),
                selected_version: Some("11.12.2".into()),
                pending: index % 2 == 0,
                updated_at: now - Duration::seconds(index as i64),
            })
            .collect();
        save_store(
            &store_path,
            &ProjectLaunchStore {
                schema_version: STORE_SCHEMA_VERSION.into(),
                records,
            },
        )
        .expect("full preference store");
        let mpr = temporary.path().join("Newest.mpr");
        fs::write(&mpr, b"fixture").expect("project fixture");
        remember_at(
            &store_path,
            &config,
            &mpr.to_string_lossy(),
            Some("11.13.0"),
            true,
        )
        .expect("remember bounded preference");
        assert_eq!(
            load_store(&store_path)
                .expect("bounded store")
                .records
                .len(),
            MAX_RECORDS
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_preference_store() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let target = temporary.path().join("target.json");
        let link = temporary.path().join("project-launches.json");
        fs::write(&target, br#"{"schemaVersion":"1.0.0","records":[]}"#).expect("target store");
        symlink(&target, &link).expect("store symlink");
        assert!(load_store(&link).is_err());
    }
}
