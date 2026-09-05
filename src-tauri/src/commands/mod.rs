mod configuration;
mod environment;
mod localization;
mod projects;
mod studio;

use crate::models::{AppConfig, CommandError, CommandErrorCode};
use tauri::AppHandle;

pub(crate) use configuration::{
    detect_settings, get_config, preview_settings_save, redetect_config, save_config,
    test_settings_connection,
};
pub(crate) use environment::{
    begin_winboat_setup, complete_winboat_setup, export_environment_diagnostic_report,
    get_capabilities, get_environment_diagnostic_report, get_environment_status, open_winboat,
    start_winboat_windows,
};
pub(crate) use localization::{
    format_localized_bytes, format_localized_dates, format_localized_numbers, get_localization,
    set_language_preference,
};
pub(crate) use projects::{
    get_projects, open_folder, select_external_project, set_project_favorite,
    set_project_launch_preference,
};
pub(crate) use studio::{
    cancel_install_queue_item, cancel_studio_download, clear_operation_history,
    enqueue_install_queue_item, fetch_downloadable_versions, get_downloadable_versions_cache,
    get_install_queue, get_installed_versions, get_installed_versions_cache, get_operations,
    get_studio_sessions, install_studio_pro, launch_studio_pro, move_install_queue_item,
    open_operation_logs, open_release_notes, reconnect_studio_session, remove_install_queue_item,
    resolve_downloadable_version, retry_install_queue_item, retry_operation, stop_studio_session,
    uninstall_studio_pro,
};

pub(super) type CommandResult<T> = Result<T, CommandError>;

pub(super) fn load_command_config(app: &AppHandle) -> CommandResult<AppConfig> {
    crate::config::load_config(app)
        .map_err(|message| CommandError::new(CommandErrorCode::ConfigLoadFailed, message))
}
