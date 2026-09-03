use super::{load_command_config, CommandResult};
use crate::app_paths::AppPaths;
use crate::contracts::StudioSessionStatus;
use crate::downloads::InstallQueue;
use crate::models::{
    CommandError, CommandErrorCode, DownloadableVersion, InstallQueueItem, InstalledVersionsCache,
    OperationRecord, StudioVersion, StudioVersionCatalog,
};
use tauri::{AppHandle, State};

#[tauri::command]
pub(crate) async fn get_installed_versions(app: AppHandle) -> CommandResult<Vec<StudioVersion>> {
    let config = load_command_config(&app)?;
    let versions = crate::platform::installed_versions(&config).await?;
    if let Ok(paths) = AppPaths::from_app(&app) {
        let _ = crate::studio_cache::save(&paths, &config, &versions);
    }
    Ok(versions)
}

#[tauri::command]
pub(crate) fn get_installed_versions_cache(
    app: AppHandle,
) -> CommandResult<InstalledVersionsCache> {
    let paths = AppPaths::from_app(&app)?;
    let config = load_command_config(&app)?;
    let cache = crate::studio_cache::load(&paths, &config).unwrap_or_default();
    #[cfg(target_os = "linux")]
    crate::winboat::seed_installed_versions_cache(&config, &cache.versions);
    Ok(cache)
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
    queue: State<'_, InstallQueue>,
    version: String,
    force_redownload: bool,
) -> CommandResult<()> {
    let item = queue
        .enqueue(version.clone(), force_redownload, false, None)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))?;
    wait_for_queue_success(&queue, &item.id).await
}

#[tauri::command]
pub(crate) fn enqueue_install_queue_item(
    queue: State<'_, InstallQueue>,
    version: String,
    force_redownload: bool,
) -> CommandResult<InstallQueueItem> {
    queue
        .enqueue(version, force_redownload, false, None)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))
}

#[tauri::command]
pub(crate) fn cancel_studio_download(queue: State<'_, InstallQueue>) -> bool {
    queue.cancel_current(true)
}

#[tauri::command]
pub(crate) fn get_install_queue(
    queue: State<'_, InstallQueue>,
) -> CommandResult<Vec<InstallQueueItem>> {
    Ok(queue.list())
}

#[tauri::command]
pub(crate) fn cancel_install_queue_item(
    queue: State<'_, InstallQueue>,
    item_id: String,
    keep_partial: bool,
) -> CommandResult<bool> {
    queue
        .cancel(&item_id, keep_partial)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))
}

#[tauri::command]
pub(crate) fn retry_install_queue_item(
    queue: State<'_, InstallQueue>,
    item_id: String,
) -> CommandResult<InstallQueueItem> {
    queue
        .retry(&item_id)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))
}

#[tauri::command]
pub(crate) fn move_install_queue_item(
    queue: State<'_, InstallQueue>,
    item_id: String,
    up: bool,
) -> CommandResult<()> {
    queue
        .move_item(&item_id, up)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))
}

#[tauri::command]
pub(crate) fn remove_install_queue_item(
    queue: State<'_, InstallQueue>,
    item_id: String,
) -> CommandResult<()> {
    queue
        .remove(&item_id)
        .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))
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
    queue: State<'_, InstallQueue>,
    id: String,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let paths = AppPaths::from_app(&app)?;
    let source = crate::operations::retry_source_with_paths(&paths, &config, &id)
        .map_err(operation_history_error)?;
    if matches!(source.kind, crate::models::OperationKind::Install) {
        let item = queue
            .enqueue(
                source.target_version.clone(),
                false,
                true,
                Some(source.id.clone()),
            )
            .map_err(|message| CommandError::new(CommandErrorCode::InvalidRequest, message))?;
        return wait_for_queue_success(&queue, &item.id).await;
    }
    let cancellation = crate::downloads::DownloadCancellation::new();
    crate::application::retry(&paths, &config, &cancellation, &id, |_| {})
        .await
        .map(|_| ())
}

async fn wait_for_queue_success(
    queue: &State<'_, InstallQueue>,
    item_id: &str,
) -> CommandResult<()> {
    let completed = queue.wait_for_terminal(item_id).await;
    match completed.state {
        crate::models::InstallQueueState::Succeeded => Ok(()),
        crate::models::InstallQueueState::Cancelled => Err(CommandError::new(
            CommandErrorCode::DownloadCancelled,
            completed
                .message
                .unwrap_or_else(|| crate::tr!("error-download-cancelled")),
        )),
        _ => Err(CommandError::new(
            CommandErrorCode::InstallFailed,
            completed
                .message
                .unwrap_or_else(|| "the Studio Pro installation failed".to_string()),
        )),
    }
}

fn operation_history_error(message: String) -> CommandError {
    CommandError::new(CommandErrorCode::OperationFailed, message)
}
