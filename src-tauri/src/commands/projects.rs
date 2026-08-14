use super::{load_command_config, CommandResult};
use crate::models::MendixProject;
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn get_projects(app: AppHandle) -> CommandResult<Vec<MendixProject>> {
    let config = load_command_config(&app)?;
    Ok(crate::projects::scan_projects(&config)?)
}

#[tauri::command]
pub(crate) fn open_linux_folder(path: String) -> CommandResult<()> {
    Ok(crate::winboat::open_linux_folder(&path)?)
}
