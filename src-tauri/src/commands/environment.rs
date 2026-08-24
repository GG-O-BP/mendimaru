use super::{load_command_config, CommandResult};
use crate::models::EnvironmentStatus;
#[cfg(target_os = "linux")]
use crate::models::SettingsSaveResult;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn get_environment_status(app: AppHandle) -> CommandResult<EnvironmentStatus> {
    let config = load_command_config(&app)?;
    Ok(crate::platform::environment_status(&config).await)
}

#[tauri::command]
#[cfg(target_os = "linux")]
pub(crate) async fn start_winboat_windows(app: AppHandle) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    crate::winboat::start_container(&config).await?;
    Ok(())
}

#[tauri::command]
#[cfg(target_os = "linux")]
pub(crate) fn open_winboat(app: AppHandle) -> CommandResult<()> {
    let config = load_command_config(&app)?;
    Ok(crate::winboat::open_winboat(&config)?)
}

#[tauri::command]
#[cfg(target_os = "linux")]
pub(crate) fn begin_winboat_setup(app: AppHandle) -> CommandResult<()> {
    let mut config = load_command_config(&app)?;
    let was_pending = config.winboat_setup_pending;
    config.winboat_setup_pending = true;
    crate::config::persist_config(&app, &config)?;
    if let Err(error) = crate::winboat::open_winboat(&config) {
        config.winboat_setup_pending = was_pending;
        let _ = crate::config::persist_config(&app, &config);
        return Err(error.into());
    }
    Ok(())
}

#[tauri::command]
#[cfg(target_os = "linux")]
pub(crate) async fn complete_winboat_setup(app: AppHandle) -> CommandResult<SettingsSaveResult> {
    let preferred = load_command_config(&app)?;
    if !preferred.winboat_setup_pending {
        return Err(crate::tr!("error-winboat-setup-not-pending").into());
    }
    if !crate::winboat::guest_is_online(&preferred).await {
        return Err(crate::tr!("error-winboat-setup-not-ready").into());
    }

    let mut detected = crate::config::detect_config()?;
    detected.language_preference = preferred.language_preference;
    detected.winboat_setup_pending = false;
    detected.shared_directory = preferred.shared_directory;
    detected.windows_shared_directory = preferred.windows_shared_directory;
    detected.mendix_install_root = preferred.mendix_install_root;
    detected.mendix_data_root = preferred.mendix_data_root;
    detected.startup_timeout_seconds = preferred.startup_timeout_seconds;

    Ok(crate::settings::save_settings(&app, detected, true).await?)
}
