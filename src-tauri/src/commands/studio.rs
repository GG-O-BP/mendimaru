use super::{load_command_config, CommandResult};
use crate::app_paths::AppPaths;
use crate::contracts::StudioSessionStatus;
use crate::downloads::{DownloadManager, DOWNLOAD_EVENT};
use crate::models::{
    CommandError, CommandErrorCode, DownloadableVersion, OperationRecord, StudioVersion,
    StudioVersionCatalog,
};
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub(crate) async fn get_installed_versions(app: AppHandle) -> CommandResult<Vec<StudioVersion>> {
    let config = load_command_config(&app)?;
    Ok(crate::platform::installed_versions(&config).await?)
}

#[tauri::command]
pub(crate) async fn get_studio_sessions(app: AppHandle) -> CommandResult<Vec<StudioSessionStatus>> {
    let config = load_command_config(&app)?;
    Ok(crate::platform::studio_sessions(&config).await?)
}

#[tauri::command]
pub(crate) async fn reconnect_studio_session(
    app: AppHandle,
    session_id: String,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    crate::application::reconnect_session(&paths, &config, &session_id).await
}

#[tauri::command]
pub(crate) async fn stop_studio_session(app: AppHandle, session_id: String) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    crate::application::stop_session(&paths, &config, &session_id).await
}

#[tauri::command]
pub(crate) fn get_downloadable_versions_cache(
    app: AppHandle,
) -> CommandResult<StudioVersionCatalog> {
    Ok(crate::marketplace::load_cached_catalog(&app)?)
}

#[tauri::command]
pub(crate) async fn fetch_downloadable_versions(
    app: AppHandle,
    page: u32,
    reset: bool,
) -> CommandResult<StudioVersionCatalog> {
    Ok(crate::marketplace::fetch_catalog_page(&app, page, reset).await?)
}

#[tauri::command]
pub(crate) async fn resolve_downloadable_version(
    app: AppHandle,
    version: String,
) -> CommandResult<DownloadableVersion> {
    Ok(crate::marketplace::resolve_downloadable_version(&app, &version).await?)
}

#[tauri::command]
pub(crate) async fn launch_studio_pro(
    app: AppHandle,
    version: String,
    project_mpr_path: Option<String>,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    crate::application::launch(&paths, &config, version, project_mpr_path.as_deref(), None)
        .await
        .map(|_| ())
}

#[tauri::command]
pub(crate) async fn uninstall_studio_pro(app: AppHandle, version: String) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    crate::application::uninstall(&paths, &config, version, None)
        .await
        .map(|_| ())
}

#[tauri::command]
pub(crate) async fn install_studio_pro(
    app: AppHandle,
    manager: State<'_, DownloadManager>,
    version: String,
    force_redownload: bool,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    crate::application::install(
        &paths,
        &config,
        &manager,
        version,
        force_redownload,
        None,
        |progress| {
            let _ = app.emit(DOWNLOAD_EVENT, progress.clone());
        },
    )
    .await
    .map(|_| ())
}

#[tauri::command]
pub(crate) fn cancel_studio_download(manager: State<'_, DownloadManager>) -> bool {
    crate::downloads::cancel_download(&manager)
}

#[tauri::command]
pub(crate) fn get_operations(app: AppHandle) -> CommandResult<Vec<OperationRecord>> {
    let config = load_command_config(&app)?;
    crate::operations::list(&app, &config).map_err(operation_history_error)
}

#[tauri::command]
pub(crate) fn clear_operation_history(app: AppHandle) -> CommandResult<usize> {
    let config = load_command_config(&app)?;
    crate::operations::clear_completed(&app, &config).map_err(operation_history_error)
}

#[tauri::command]
pub(crate) fn open_operation_logs(app: AppHandle) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let directory = crate::operations::log_directory(&config).map_err(operation_history_error)?;
    let directory = directory.to_str().ok_or_else(|| {
        operation_history_error("the operation log path is not valid UTF-8".into())
    })?;
    crate::platform::open_folder(directory).map_err(CommandError::from)
}

#[tauri::command]
pub(crate) async fn retry_operation(
    app: AppHandle,
    manager: State<'_, DownloadManager>,
    id: String,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    crate::application::retry(&paths, &config, &manager, &id, |progress| {
        let _ = app.emit(DOWNLOAD_EVENT, progress.clone());
    })
    .await
    .map(|_| ())
}

fn operation_history_error(message: String) -> CommandError {
    CommandError::new(CommandErrorCode::OperationFailed, message)
}
