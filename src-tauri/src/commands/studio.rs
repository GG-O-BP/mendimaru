use super::{load_command_config, CommandResult};
use crate::contracts::StudioSessionStatus;
use crate::downloads::{DownloadManager, InstallError};
use crate::models::{
    CommandError, CommandErrorCode, DownloadProgress, DownloadState, OperationKind,
    OperationRecord, OperationStage, StudioVersion, StudioVersionCatalog,
};
use crate::operations::{OperationTracker, SessionActionGuard};
use tauri::{AppHandle, State};

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
    let session = crate::platform::studio_session_status(&config, &session_id).await?;
    let _guard = SessionActionGuard::begin(&app, &config, &session.version)
        .map_err(session_conflict_error)?;
    Ok(crate::platform::reconnect_studio_session(&config, &session_id).await?)
}

#[tauri::command]
pub(crate) async fn stop_studio_session(app: AppHandle, session_id: String) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    let session = crate::platform::studio_session_status(&config, &session_id).await?;
    let _guard = SessionActionGuard::begin(&app, &config, &session.version)
        .map_err(session_conflict_error)?;
    Ok(crate::platform::stop_studio_session(&config, &session_id).await?)
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
pub(crate) async fn launch_studio_pro(
    app: AppHandle,
    version: String,
    project_mpr_path: Option<String>,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    run_launch(&app, &config, version, project_mpr_path.as_deref(), None).await
}

#[tauri::command]
pub(crate) async fn uninstall_studio_pro(app: AppHandle, version: String) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    run_uninstall(&app, &config, version, None).await
}

#[tauri::command]
pub(crate) async fn install_studio_pro(
    app: AppHandle,
    manager: State<'_, DownloadManager>,
    version: String,
    force_redownload: bool,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    run_install(&app, &config, &manager, version, force_redownload, None).await
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
    let source =
        crate::operations::retry_source(&app, &config, &id).map_err(operation_history_error)?;
    let retry_of = Some(source.id.clone());
    match source.kind {
        OperationKind::Install => {
            run_install(
                &app,
                &config,
                &manager,
                source.target_version,
                false,
                retry_of,
            )
            .await
        }
        OperationKind::Uninstall => {
            run_uninstall(&app, &config, source.target_version, retry_of).await
        }
        OperationKind::Launch if !source.protected_project => {
            run_launch(&app, &config, source.target_version, None, retry_of).await
        }
        OperationKind::Launch => Err(CommandError::new(
            CommandErrorCode::InvalidRequest,
            "project launches cannot be retried without selecting the project again".into(),
        )),
    }
}

async fn run_launch(
    app: &AppHandle,
    config: &crate::models::AppConfig,
    version: String,
    project_mpr_path: Option<&str>,
    retry_of: Option<String>,
) -> CommandResult<()> {
    let protected_project = project_mpr_path.is_some();
    let tracker = OperationTracker::begin(
        app,
        config,
        OperationKind::Launch,
        &version,
        protected_project,
        OperationStage::Launching,
        retry_of,
    )
    .map_err(operation_history_error)?;
    let result = crate::platform::launch_studio(config, &version, tracker.id(), project_mpr_path)
        .await
        .map_err(CommandError::from);
    complete_operation(tracker, result)
}

async fn run_uninstall(
    app: &AppHandle,
    config: &crate::models::AppConfig,
    version: String,
    retry_of: Option<String>,
) -> CommandResult<()> {
    ensure_no_running_session(config, &version).await?;
    let tracker = OperationTracker::begin(
        app,
        config,
        OperationKind::Uninstall,
        &version,
        false,
        OperationStage::Uninstalling,
        retry_of,
    )
    .map_err(operation_history_error)?;
    let result = crate::platform::uninstall_studio(config, &version, tracker.id())
        .await
        .map_err(CommandError::from);
    complete_operation(tracker, result)
}

async fn run_install(
    app: &AppHandle,
    config: &crate::models::AppConfig,
    manager: &DownloadManager,
    version: String,
    force_redownload: bool,
    retry_of: Option<String>,
) -> CommandResult<()> {
    ensure_no_running_session(config, &version).await?;
    let mut tracker = OperationTracker::begin(
        app,
        config,
        OperationKind::Install,
        &version,
        false,
        OperationStage::Starting,
        retry_of,
    )
    .map_err(operation_history_error)?;
    let operation_id = tracker.id().to_string();
    let result = crate::downloads::download_and_launch(
        app,
        config,
        manager,
        version,
        &operation_id,
        force_redownload,
        |progress| {
            let _ = tracker.progress(
                operation_stage(progress),
                progress.percentage,
                progress.estimated,
            );
        },
    )
    .await;
    match result {
        Ok(()) => tracker.succeed().map_err(operation_history_error),
        Err(InstallError::Cancelled(message)) => {
            let command_error = CommandError::new(CommandErrorCode::DownloadCancelled, message);
            let _ = tracker.cancel("download_cancelled");
            Err(command_error)
        }
        Err(error) => {
            let command_error = install_error(error);
            let _ = tracker.fail(&command_error);
            Err(command_error)
        }
    }
}

async fn ensure_no_running_session(
    config: &crate::models::AppConfig,
    version: &str,
) -> CommandResult<()> {
    if crate::platform::studio_sessions(config)
        .await?
        .iter()
        .any(|session| session.version == version)
    {
        return Err(CommandError::new(
            CommandErrorCode::PreconditionFailed,
            crate::tr!("error-studio-session-version-busy", version = version),
        ));
    }
    Ok(())
}

fn session_conflict_error(_message: String) -> CommandError {
    CommandError::new(
        CommandErrorCode::PreconditionFailed,
        crate::tr!("error-studio-session-operation-conflict"),
    )
}

fn complete_operation(tracker: OperationTracker, result: CommandResult<()>) -> CommandResult<()> {
    match result {
        Ok(()) => tracker.succeed().map_err(operation_history_error),
        Err(error) => {
            let _ = tracker.fail(&error);
            Err(error)
        }
    }
}

fn install_error(error: InstallError) -> CommandError {
    match error {
        InstallError::Cancelled(message) => {
            CommandError::new(CommandErrorCode::DownloadCancelled, message)
        }
        InstallError::Backend(error) => error.into(),
        InstallError::Other(message) => CommandError::new(CommandErrorCode::InstallFailed, message),
    }
}

fn operation_stage(progress: &DownloadProgress) -> OperationStage {
    match progress.state {
        DownloadState::Starting => OperationStage::Starting,
        DownloadState::Preparing => OperationStage::Preparing,
        DownloadState::Checking => OperationStage::Checking,
        DownloadState::Connecting => OperationStage::Connecting,
        DownloadState::Downloading => OperationStage::Downloading,
        DownloadState::Downloaded => OperationStage::Downloaded,
        DownloadState::Ready => OperationStage::Ready,
        DownloadState::Staging => OperationStage::Staging,
        DownloadState::Installing => OperationStage::Installing,
        DownloadState::Finalizing => OperationStage::Finalizing,
        DownloadState::Verifying => OperationStage::Verifying,
        DownloadState::Installed => OperationStage::Completed,
        DownloadState::Failed | DownloadState::Cancelled => OperationStage::Interrupted,
    }
}

fn operation_history_error(message: String) -> CommandError {
    CommandError::new(CommandErrorCode::OperationFailed, message)
}
