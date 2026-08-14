use super::{load_command_config, CommandResult};
use crate::models::{AppConfig, SettingsSaveResult};
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn get_config(app: AppHandle) -> CommandResult<AppConfig> {
    load_command_config(&app)
}

#[tauri::command]
pub(crate) fn redetect_config(app: AppHandle) -> CommandResult<AppConfig> {
    let current = crate::config::load_config(&app).ok();
    let mut detected = crate::config::detect_config()?;
    detected.language_preference = current
        .as_ref()
        .map(|config| config.language_preference.clone())
        .unwrap_or_else(|| "system".to_string());
    detected.winboat_setup_pending = current
        .as_ref()
        .is_some_and(|config| config.winboat_setup_pending);
    crate::config::persist_config(&app, &detected)?;
    Ok(detected)
}

#[tauri::command]
pub(crate) async fn save_config(
    app: AppHandle,
    config: AppConfig,
    apply_mount: bool,
) -> CommandResult<SettingsSaveResult> {
    Ok(crate::settings::save_settings(&app, config, apply_mount).await?)
}
