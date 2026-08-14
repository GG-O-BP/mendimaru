mod app_paths;
mod application;
pub mod cli;
mod commands;
#[cfg_attr(target_os = "windows", allow(dead_code))]
mod config;
pub mod contracts;
mod downloads;
mod i18n;
mod marketplace;
pub mod models;
mod operations;
pub mod platform;
mod project_launches;
mod projects;
mod settings;
#[cfg_attr(target_os = "windows", allow(dead_code, unused_imports))]
mod winboat;

use commands::*;
use downloads::DownloadManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            i18n::initialize("system").map_err(std::io::Error::other)?;
            if let Ok(config) = config::load_config(app.handle()) {
                i18n::set_language(&config.language_preference).map_err(std::io::Error::other)?;
                let _ = operations::list(app.handle(), &config);
            }
            Ok(())
        })
        .manage(DownloadManager::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_localization,
            set_language_preference,
            format_localized_dates,
            format_localized_numbers,
            format_localized_bytes,
            redetect_config,
            save_config,
            get_capabilities,
            get_environment_status,
            get_environment_diagnostic_report,
            export_environment_diagnostic_report,
            get_installed_versions,
            get_studio_sessions,
            get_downloadable_versions_cache,
            fetch_downloadable_versions,
            resolve_downloadable_version,
            get_projects,
            set_project_launch_preference,
            start_winboat_windows,
            open_winboat,
            begin_winboat_setup,
            complete_winboat_setup,
            launch_studio_pro,
            reconnect_studio_session,
            stop_studio_session,
            uninstall_studio_pro,
            install_studio_pro,
            cancel_studio_download,
            get_operations,
            retry_operation,
            clear_operation_history,
            open_operation_logs,
            open_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mendimaru");
}
