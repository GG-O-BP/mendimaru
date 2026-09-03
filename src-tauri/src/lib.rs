// MSVC emits a localized informational line while creating the cdylib import library.
#![cfg_attr(target_os = "windows", allow(linker_messages))]

mod app_paths;
mod application;
mod browser;
pub mod cli;
mod commands;
#[cfg_attr(target_os = "windows", allow(dead_code))]
mod config;
pub mod contracts;
mod downloads;
#[cfg(feature = "e2e")]
mod e2e;
mod i18n;
mod install_queue_host;
mod marketplace;
pub mod models;
mod operations;
pub mod platform;
mod portable_runtime;
pub mod process;
mod project_launches;
mod project_watcher;
mod projects;
mod settings;
mod studio_cache;
mod studio_trace;
#[cfg_attr(target_os = "windows", allow(dead_code, unused_imports))]
mod winboat;

use commands::*;
use downloads::InstallQueue;
use install_queue_host::TauriInstallQueueHost;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    studio_trace::enable_desktop();
    let builder = tauri::Builder::default()
        .manage(InstallQueue::new())
        .setup(|app| {
            #[cfg(feature = "e2e")]
            crate::e2e::require_isolated_root().map_err(std::io::Error::other)?;
            i18n::initialize("system").map_err(std::io::Error::other)?;
            if let Ok(config) = config::load_config(app.handle()) {
                i18n::set_language(&config.language_preference).map_err(std::io::Error::other)?;
                let _ = operations::list(app.handle(), &config);
            }
            let queue = app.state::<InstallQueue>();
            queue.set_host(Arc::new(TauriInstallQueueHost::new(app.handle().clone())));
            if let Ok(paths) = app_paths::AppPaths::from_app(app.handle()) {
                if let Err(error) =
                    queue.restore(paths.config_directory().join("install-queue.json"))
                {
                    eprintln!("install queue restore failed: {error}");
                }
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());
    let app = builder
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_localization,
            set_language_preference,
            format_localized_dates,
            format_localized_numbers,
            format_localized_bytes,
            detect_settings,
            redetect_config,
            preview_settings_save,
            save_config,
            test_settings_connection,
            get_capabilities,
            get_environment_status,
            get_environment_diagnostic_report,
            export_environment_diagnostic_report,
            get_installed_versions,
            get_installed_versions_cache,
            get_studio_sessions,
            get_downloadable_versions_cache,
            fetch_downloadable_versions,
            resolve_downloadable_version,
            get_projects,
            select_external_project,
            set_project_launch_preference,
            set_project_favorite,
            start_winboat_windows,
            open_winboat,
            begin_winboat_setup,
            complete_winboat_setup,
            launch_studio_pro,
            reconnect_studio_session,
            stop_studio_session,
            uninstall_studio_pro,
            install_studio_pro,
            enqueue_install_queue_item,
            cancel_studio_download,
            get_install_queue,
            cancel_install_queue_item,
            retry_install_queue_item,
            move_install_queue_item,
            remove_install_queue_item,
            get_operations,
            retry_operation,
            clear_operation_history,
            open_operation_logs,
            open_folder,
            #[cfg(all(feature = "e2e", target_os = "windows"))]
            e2e::e2e_bounded_process_cleanup,
        ])
        .build(tauri::generate_context!())
        .expect("error while building mendimaru");
    let exit_code = app.run_return(|_, _| {});
    #[cfg(target_os = "linux")]
    tauri::async_runtime::block_on(winboat::close_all_registered_clients());
    std::process::exit(exit_code);
}
