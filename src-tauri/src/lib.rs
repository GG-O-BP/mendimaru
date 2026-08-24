// MSVC emits a localized informational line while creating the cdylib import library.
#![cfg_attr(target_os = "windows", allow(linker_messages))]

mod commands;
#[cfg_attr(target_os = "windows", allow(dead_code))]
mod config;
mod downloads;
#[cfg(feature = "e2e")]
mod e2e;
mod i18n;
mod marketplace;
mod models;
mod platform;
mod projects;
mod settings;
#[cfg(target_os = "linux")]
mod winboat;

use commands::*;
use downloads::DownloadManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .setup(|app| {
            #[cfg(feature = "e2e")]
            crate::e2e::require_isolated_root().map_err(std::io::Error::other)?;
            i18n::initialize("system").map_err(std::io::Error::other)?;
            if let Ok(config) = config::load_config(app.handle()) {
                i18n::set_language(&config.language_preference).map_err(std::io::Error::other)?;
            }
            Ok(())
        })
        .manage(DownloadManager::default())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());
    #[cfg(target_os = "linux")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_config,
        get_localization,
        set_language_preference,
        format_localized_dates,
        format_localized_numbers,
        format_localized_bytes,
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
        open_folder,
    ]);
    #[cfg(not(target_os = "linux"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_config,
        get_localization,
        set_language_preference,
        format_localized_dates,
        format_localized_numbers,
        format_localized_bytes,
        redetect_config,
        save_config,
        get_environment_status,
        get_installed_versions,
        get_downloadable_versions_cache,
        fetch_downloadable_versions,
        get_projects,
        launch_studio_pro,
        uninstall_studio_pro,
        install_studio_pro,
        cancel_studio_download,
        open_folder,
    ]);
    builder
        .run(tauri::generate_context!())
        .expect("error while running mendimaru");
}
