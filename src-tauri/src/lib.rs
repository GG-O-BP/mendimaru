mod config;
mod downloads;
mod i18n;
mod marketplace;
mod models;
mod projects;
mod winboat;

use downloads::DownloadManager;
use models::{
    AppConfig, CommandError, EnvironmentStatus, InstallResult, LaunchResult, LocalizationBundle,
    MendixProject, SettingsSaveResult, StudioVersion, StudioVersionCatalog,
};
use tauri::{AppHandle, State};

#[tauri::command]
fn get_config(app: AppHandle) -> Result<AppConfig, String> {
    config::load_config(&app)
}

#[tauri::command]
fn get_localization(app: AppHandle) -> Result<LocalizationBundle, String> {
    let config = config::load_config(&app)?;
    let preference = i18n::normalize_preference(&config.language_preference)?;
    i18n::set_language(&preference)?;
    Ok(i18n::bundle(&preference))
}

#[tauri::command]
fn set_language_preference(app: AppHandle, language: String) -> Result<LocalizationBundle, String> {
    let preference = i18n::normalize_preference(&language)?;
    let mut config = config::load_config(&app)?;
    let previous_preference = config.language_preference.clone();
    i18n::set_language(&preference)?;
    config.language_preference = preference.clone();
    if let Err(error) = config::persist_config(&app, &config) {
        let _ = i18n::set_language(&previous_preference);
        return Err(error);
    }
    Ok(i18n::bundle(&preference))
}

#[tauri::command]
fn format_localized_dates(values: Vec<String>) -> Vec<String> {
    i18n::format_dates(&values)
}

#[tauri::command]
fn format_localized_numbers(values: Vec<u64>) -> Vec<String> {
    i18n::format_numbers(&values)
}

#[tauri::command]
fn format_localized_bytes(values: Vec<u64>) -> Vec<String> {
    values.into_iter().map(i18n::format_bytes).collect()
}

#[tauri::command]
fn format_localized_duration(total_seconds: u64) -> String {
    i18n::format_duration(total_seconds)
}

#[tauri::command]
fn redetect_config(app: AppHandle) -> Result<AppConfig, String> {
    let current = config::load_config(&app).ok();
    let mut detected = config::detect_config()?;
    detected.language_preference = current
        .as_ref()
        .map(|config| config.language_preference.clone())
        .unwrap_or_else(|| "system".to_string());
    detected.winboat_setup_pending = current
        .as_ref()
        .is_some_and(|config| config.winboat_setup_pending);
    config::persist_config(&app, &detected)?;
    Ok(detected)
}

#[tauri::command]
async fn save_config(
    app: AppHandle,
    config: AppConfig,
    apply_mount: bool,
) -> Result<SettingsSaveResult, String> {
    config::save_settings(&app, config, apply_mount).await
}

#[tauri::command]
async fn get_environment_status(app: AppHandle) -> Result<EnvironmentStatus, String> {
    let config = config::load_config(&app)?;
    Ok(winboat::environment_status(&config).await)
}

#[tauri::command]
async fn get_installed_versions(app: AppHandle) -> Result<Vec<StudioVersion>, String> {
    let config = config::load_config(&app)?;
    winboat::installed_versions(&config).await
}

#[tauri::command]
fn get_downloadable_versions_cache(app: AppHandle) -> Result<StudioVersionCatalog, String> {
    marketplace::load_cached_catalog(&app)
}

#[tauri::command]
async fn fetch_downloadable_versions(
    app: AppHandle,
    page: u32,
    reset: bool,
) -> Result<StudioVersionCatalog, String> {
    marketplace::fetch_catalog_page(&app, page, reset).await
}

#[tauri::command]
fn get_projects(app: AppHandle) -> Result<Vec<MendixProject>, String> {
    let config = config::load_config(&app)?;
    projects::scan_projects(&config)
}

#[tauri::command]
async fn start_winboat_windows(app: AppHandle) -> Result<String, String> {
    let config = config::load_config(&app)?;
    winboat::start_container(&config).await
}

#[tauri::command]
fn open_winboat(app: AppHandle) -> Result<(), String> {
    let config = config::load_config(&app)?;
    winboat::open_winboat(&config)
}

#[tauri::command]
fn begin_winboat_setup(app: AppHandle) -> Result<(), String> {
    let mut config = config::load_config(&app)?;
    let was_pending = config.winboat_setup_pending;
    config.winboat_setup_pending = true;
    config::persist_config(&app, &config)?;
    if let Err(error) = winboat::open_winboat(&config) {
        config.winboat_setup_pending = was_pending;
        let _ = config::persist_config(&app, &config);
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
async fn complete_winboat_setup(app: AppHandle) -> Result<SettingsSaveResult, String> {
    let preferred = config::load_config(&app)?;
    if !preferred.winboat_setup_pending {
        return Err(crate::tr!("error-winboat-setup-not-pending"));
    }
    if !winboat::guest_is_online(&preferred).await {
        return Err(crate::tr!("error-winboat-setup-not-ready"));
    }

    let mut detected = config::detect_config()?;
    detected.language_preference = preferred.language_preference;
    detected.winboat_setup_pending = true;
    detected.shared_directory = preferred.shared_directory;
    detected.windows_shared_directory = preferred.windows_shared_directory;
    detected.mendix_install_root = preferred.mendix_install_root;
    detected.mendix_data_root = preferred.mendix_data_root;
    detected.startup_timeout_seconds = preferred.startup_timeout_seconds;

    let mut result = config::save_settings(&app, detected, true).await?;
    result.config.winboat_setup_pending = false;
    config::persist_config(&app, &result.config)?;
    Ok(result)
}

#[tauri::command]
async fn launch_studio_pro(
    app: AppHandle,
    version: String,
    project_mpr_path: Option<String>,
) -> Result<LaunchResult, String> {
    let config = config::load_config(&app)?;
    winboat::launch_studio(&config, &version, project_mpr_path.as_deref()).await
}

#[tauri::command]
async fn uninstall_studio_pro(app: AppHandle, version: String) -> Result<(), String> {
    let config = config::load_config(&app)?;
    winboat::launch_uninstaller(&config, &version).await
}

#[tauri::command]
async fn install_studio_pro(
    app: AppHandle,
    manager: State<'_, DownloadManager>,
    version: String,
) -> Result<InstallResult, CommandError> {
    let config = config::load_config(&app)
        .map_err(|message| CommandError::new("config_load_failed", message))?;
    downloads::download_and_launch(&app, &config, &manager, version)
        .await
        .map_err(|error| match error {
            downloads::InstallError::Cancelled(message) => {
                CommandError::new("download_cancelled", message)
            }
            downloads::InstallError::Other(message) => CommandError::new("install_failed", message),
        })
}

#[tauri::command]
fn cancel_studio_download(manager: State<'_, DownloadManager>) -> bool {
    downloads::cancel_download(&manager)
}

#[tauri::command]
fn open_linux_folder(path: String) -> Result<(), String> {
    winboat::open_linux_folder(&path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            i18n::initialize("system").map_err(std::io::Error::other)?;
            if let Ok(config) = config::load_config(app.handle()) {
                i18n::set_language(&config.language_preference).map_err(std::io::Error::other)?;
            }
            Ok(())
        })
        .manage(DownloadManager::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_localization,
            set_language_preference,
            format_localized_dates,
            format_localized_numbers,
            format_localized_bytes,
            format_localized_duration,
            redetect_config,
            save_config,
            get_environment_status,
            get_installed_versions,
            get_downloadable_versions_cache,
            fetch_downloadable_versions,
            get_projects,
            start_winboat_windows,
            open_winboat,
            begin_winboat_setup,
            complete_winboat_setup,
            launch_studio_pro,
            uninstall_studio_pro,
            install_studio_pro,
            cancel_studio_download,
            open_linux_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mendimaru");
}
