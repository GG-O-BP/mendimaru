use super::{load_command_config, CommandResult};
use crate::models::{CommandError, CommandErrorCode, MendixProject};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

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
