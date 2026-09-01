use super::{load_command_config, CommandResult};
use crate::models::{AppConfig, CommandError, CommandErrorCode, MendixProject, ProjectScanResult};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

static SCAN_EPOCH: AtomicU64 = AtomicU64::new(0);
static ACTIVE_SCANS: OnceLock<Mutex<HashMap<u64, Arc<AtomicBool>>>> = OnceLock::new();
static LATEST_SCAN: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub(crate) async fn get_projects(app: AppHandle) -> CommandResult<ProjectScanResult> {
    let config = load_command_config(&app)?;
    let (scan_id, cancellation) = begin_project_scan();
    let scan_app = app.clone();
    let scan_result = tauri::async_runtime::spawn_blocking(move || {
        let watcher_active = crate::project_watcher::refresh(&scan_app, &config);
        let mut result = crate::projects::scan_projects_result(&config, &cancellation)?;
        if cancellation.load(Ordering::Relaxed) {
            return Err("workspace scan superseded by a newer request".to_string());
        }
        result.projects =
            apply_preferences_if_current(&scan_app, &config, scan_id, result.projects)?;
        result.watcher_active = watcher_active;
        Ok(result)
    })
    .await
    .map_err(|error| format!("workspace scan stopped unexpectedly: {error}"))?
    .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message));
    end_project_scan(scan_id);
    scan_result
}

fn begin_project_scan() -> (u64, Arc<AtomicBool>) {
    let scan_id = SCAN_EPOCH.fetch_add(1, Ordering::Relaxed);
    let cancellation = Arc::new(AtomicBool::new(false));
    let scans = ACTIVE_SCANS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut scans) = scans.lock() {
        for active in scans.values() {
            active.store(true, Ordering::Relaxed);
        }
        LATEST_SCAN.store(scan_id, Ordering::Relaxed);
        scans.insert(scan_id, cancellation.clone());
    }
    (scan_id, cancellation)
}

fn apply_preferences_if_current(
    app: &AppHandle,
    config: &AppConfig,
    scan_id: u64,
    mut projects: Vec<MendixProject>,
) -> Result<Vec<MendixProject>, String> {
    let scans = ACTIVE_SCANS.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(scans) = scans.lock() else {
        return Err("workspace scan state is unavailable".to_string());
    };
    if LATEST_SCAN.load(Ordering::Relaxed) != scan_id || !scans.contains_key(&scan_id) {
        return Err("workspace scan superseded by a newer request".to_string());
    }
    crate::project_launches::apply_preferences(app, config, &mut projects)?;
    Ok(projects)
}

fn end_project_scan(scan_id: u64) {
    let Some(scans) = ACTIVE_SCANS.get() else {
        return;
    };
    if let Ok(mut scans) = scans.lock() {
        scans.remove(&scan_id);
    }
}

#[tauri::command]
pub(crate) fn set_project_launch_preference(
    app: AppHandle,
    project_mpr_path: String,
    selected_version: Option<String>,
    pending: bool,
    completed_launch: Option<bool>,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    Ok(crate::project_launches::remember(
        &app,
        &config,
        &project_mpr_path,
        selected_version.as_deref(),
        pending,
        completed_launch.unwrap_or(false),
    )?)
}

#[tauri::command]
pub(crate) fn set_project_favorite(
    app: AppHandle,
    project_mpr_path: String,
    favorite: bool,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    Ok(crate::project_launches::set_favorite(
        &app,
        &config,
        &project_mpr_path,
        favorite,
    )?)
}

#[tauri::command]
pub(crate) fn select_external_project(app: AppHandle) -> CommandResult<Option<MendixProject>> {
    if !cfg!(target_os = "linux") {
        return Err(CommandError::new(
            CommandErrorCode::UnsupportedCapability,
            crate::tr!("error-external-project-linux-only"),
        ));
    }
    let config = load_command_config(&app)?;
    let selection = app
        .dialog()
        .file()
        .add_filter("Mendix project", &["mpr"])
        .blocking_pick_file();
    let Some(selection) = selection else {
        return Ok(None);
    };
    let path = selection
        .into_path()
        .map_err(|error| crate::tr!("error-project-path-encoding-detail", error = error))?;
    let mut project = crate::projects::inspect_selected_project(&config, &path)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))?;
    crate::project_launches::apply_preferences(&app, &config, std::slice::from_mut(&mut project))?;
    Ok(Some(project))
}

#[tauri::command]
pub(crate) fn open_folder(path: String) -> CommandResult<()> {
    Ok(crate::platform::open_folder(&path)?)
}
