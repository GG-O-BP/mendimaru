use super::{load_command_config, CommandResult};
use crate::models::{AppConfig, SettingsSavePreview, SettingsSaveResult};
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
    if crate::platform::is_windows_native() {
        if let Some(current) = current.as_ref() {
            if std::path::Path::new(&current.shared_directory).is_dir() {
                detected.shared_directory = current.shared_directory.clone();
            }
            detected.windows_studio_paths = current.windows_studio_paths.clone();
        }
    }
    crate::config::persist_config(&app, &detected)?;
    Ok(detected)
}

#[tauri::command]
pub(crate) fn preview_settings_save(
    config: AppConfig,
    apply_mount: bool,
) -> CommandResult<Option<SettingsSavePreview>> {
    crate::settings::preview_settings_save(config, apply_mount)
}

#[tauri::command]
pub(crate) async fn save_config(
    app: AppHandle,
    config: AppConfig,
    apply_mount: bool,
    compose_revision: Option<String>,
) -> CommandResult<SettingsSaveResult> {
    crate::settings::save_settings(&app, config, apply_mount, compose_revision).await
}
