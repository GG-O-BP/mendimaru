use super::{load_command_config, CommandResult};
use crate::downloads::{DownloadManager, InstallError};
use crate::models::{CommandError, CommandErrorCode, StudioVersion, StudioVersionCatalog};
use tauri::{AppHandle, State};

#[tauri::command]
pub(crate) async fn get_installed_versions(app: AppHandle) -> CommandResult<Vec<StudioVersion>> {
    let config = load_command_config(&app)?;
    Ok(crate::platform::installed_versions(&config).await?)
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
    Ok(crate::platform::launch_studio(&config, &version, project_mpr_path.as_deref()).await?)
}

#[tauri::command]
pub(crate) async fn uninstall_studio_pro(app: AppHandle, version: String) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    Ok(crate::platform::uninstall_studio(&config, &version).await?)
}

#[tauri::command]
pub(crate) async fn install_studio_pro(
    app: AppHandle,
    manager: State<'_, DownloadManager>,
    version: String,
    force_redownload: bool,
) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    crate::downloads::download_and_launch(&app, &config, &manager, version, force_redownload)
        .await
        .map_err(|error| match error {
            InstallError::Cancelled(message) => {
                CommandError::new(CommandErrorCode::DownloadCancelled, message)
            }
            InstallError::Backend(error) => error.into(),
            InstallError::Other(message) => {
                CommandError::new(CommandErrorCode::InstallFailed, message)
            }
        })
}

#[tauri::command]
pub(crate) fn cancel_studio_download(manager: State<'_, DownloadManager>) -> bool {
    crate::downloads::cancel_download(&manager)
}
