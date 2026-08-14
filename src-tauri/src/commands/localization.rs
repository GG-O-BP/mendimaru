use super::{load_command_config, CommandResult};
use crate::models::LocalizationBundle;
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn get_localization(app: AppHandle) -> CommandResult<LocalizationBundle> {
    let preference = crate::config::load_config(&app)
        .ok()
        .and_then(|config| crate::i18n::normalize_preference(&config.language_preference).ok())
        .unwrap_or_else(|| "system".to_string());
    crate::i18n::set_language(&preference)?;
    Ok(crate::i18n::bundle(&preference))
}

#[tauri::command]
pub(crate) fn set_language_preference(
    app: AppHandle,
    language: String,
) -> CommandResult<LocalizationBundle> {
    let preference = crate::i18n::normalize_preference(&language)?;
    let mut config = load_command_config(&app)?;
    let previous_preference = config.language_preference.clone();
    crate::i18n::set_language(&preference)?;
    config.language_preference = preference.clone();
    if let Err(error) = crate::config::persist_config(&app, &config) {
        let _ = crate::i18n::set_language(&previous_preference);
        return Err(error.into());
    }
    Ok(crate::i18n::bundle(&preference))
}

#[tauri::command]
pub(crate) fn format_localized_dates(values: Vec<String>) -> Vec<String> {
    crate::i18n::format_dates(&values)
}

#[tauri::command]
pub(crate) fn format_localized_numbers(values: Vec<u64>) -> Vec<String> {
    crate::i18n::format_numbers(&values)
}

#[tauri::command]
pub(crate) fn format_localized_bytes(values: Vec<u64>) -> Vec<String> {
    values.into_iter().map(crate::i18n::format_bytes).collect()
}
