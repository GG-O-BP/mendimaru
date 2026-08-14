use super::{load_command_config, CommandResult};
use crate::models::MendixProject;
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn get_projects(app: AppHandle) -> CommandResult<Vec<MendixProject>> {
    let config = load_command_config(&app)?;
    let mut projects = crate::projects::scan_projects(&config)?;
    crate::project_launches::apply_preferences(&app, &config, &mut projects)?;
    Ok(projects)
}

#[tauri::command]
pub(crate) fn set_project_launch_preference(
    app: AppHandle,
    project_mpr_path: String,
    selected_version: Option<String>,
    pending: bool,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    Ok(crate::project_launches::remember(
        &app,
        &config,
        &project_mpr_path,
        selected_version.as_deref(),
        pending,
    )?)
}

#[tauri::command]
pub(crate) fn open_folder(path: String) -> CommandResult<()> {
    Ok(crate::platform::open_folder(&path)?)
}
