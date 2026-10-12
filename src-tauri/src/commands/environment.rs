use super::{load_command_config, CommandResult};
use crate::contracts::{BackendId, CapabilitySnapshot};
use crate::models::{EnvironmentStatus, SettingsSaveResult};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub(crate) fn get_capabilities(backend: Option<BackendId>) -> CommandResult<CapabilitySnapshot> {
    Ok(crate::platform::capability_snapshot(backend)?)
}

#[tauri::command]
pub(crate) async fn get_environment_status(app: AppHandle) -> CommandResult<EnvironmentStatus> {
    #[cfg(feature = "e2e")]
    crate::e2e::delay_environment_status_for_performance().await?;
    let config = load_command_config(&app)?;
    Ok(crate::platform::environment_status(&config).await)
}

#[tauri::command]
pub(crate) async fn get_environment_diagnostic_report(app: AppHandle) -> CommandResult<String> {
    let config = load_command_config(&app)?;
    let status = crate::platform::environment_status(&config).await;
    Ok(crate::models::environment_diagnostic_report(&status)?)
}

#[tauri::command]
pub(crate) async fn export_environment_diagnostic_report(app: AppHandle) -> CommandResult<bool> {
    let config = load_command_config(&app)?;
    let status = crate::platform::environment_status(&config).await;
    let report = crate::models::environment_diagnostic_report(&status)?;
    let selection = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name("mendimaru-environment-report.json")
        .blocking_save_file();
    let Some(selection) = selection else {
        return Ok(false);
    };
    let path = selection
        .into_path()
        .map_err(|error| crate::tr!("error-environment-report-path", error = error))?;
    std::fs::write(&path, report)
        .map_err(|error| crate::tr!("error-environment-report-write", error = error))?;
    Ok(true)
}

#[tauri::command]
pub(crate) async fn start_winboat_windows(app: AppHandle) -> CommandResult<()> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    crate::winboat::ensure_guest_online(&config).await?;
    Ok(())
}

#[tauri::command]
pub(crate) fn open_winboat(app: AppHandle) -> CommandResult<()> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    Ok(crate::winboat::open_winboat(&config)?)
}

#[tauri::command]
pub(crate) fn begin_winboat_setup(app: AppHandle) -> CommandResult<()> {
    require_winboat()?;
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
pub(crate) async fn complete_winboat_setup(app: AppHandle) -> CommandResult<SettingsSaveResult> {
    require_winboat()?;
    let paths = crate::app_paths::AppPaths::from_app(&app)?;
    complete_winboat_setup_from(&paths).await
}

pub(crate) async fn complete_winboat_setup_from(
    paths: &crate::app_paths::AppPaths,
) -> CommandResult<SettingsSaveResult> {
    let preferred = crate::config::load_config_from(paths)?;
    let detected = crate::config::detect_config()?;
    complete_winboat_setup_with_config(paths, preferred, detected).await
}

async fn complete_winboat_setup_with_config(
    paths: &crate::app_paths::AppPaths,
    preferred: crate::models::AppConfig,
    mut detected: crate::models::AppConfig,
) -> CommandResult<SettingsSaveResult> {
    if !preferred.winboat_setup_pending {
        return Err(crate::tr!("error-winboat-setup-not-pending").into());
    }
    #[cfg(target_os = "linux")]
    {
        crate::winboat::maintenance::validate_identity(&preferred).map_err(|error| {
            error.command_error(crate::contracts::CapabilityId::EnvironmentConfigure)
        })?;
        // A pending configuration can already select a healthy custom VM.
        // Finishing its setup must not silently retarget the default ~/.winboat
        // VM found by auto-detection (especially with CLI/config overrides).
        if std::fs::symlink_metadata(&preferred.compose_file).is_ok() {
            detected.compose_file = preferred.compose_file.clone();
            detected.container_runtime = preferred.container_runtime;
            detected.container_name = preferred.container_name.clone();
            detected.api_url = preferred.api_url.clone();
            detected.rdp_host = preferred.rdp_host.clone();
            detected.rdp_port = preferred.rdp_port;
        }
    }
    if !crate::winboat::guest_is_online(&preferred).await {
        return Err(crate::tr!("error-winboat-setup-not-ready").into());
    }
    detected.language_preference = preferred.language_preference;
    detected.winboat_setup_pending = false;
    detected.shared_directory = preferred.shared_directory;
    detected.windows_shared_directory = preferred.windows_shared_directory;
    detected.mendix_install_root = preferred.mendix_install_root;
    detected.mendix_data_root = preferred.mendix_data_root;
    detected.startup_timeout_seconds = preferred.startup_timeout_seconds;
    let revision = crate::settings::preview_settings_save(detected.clone(), true)?
        .map(|preview| preview.compose_revision);
    crate::settings::save_settings_from(paths, detected, true, revision).await
}

fn require_winboat() -> CommandResult<()> {
    if crate::platform::capabilities().requires_winboat {
        Ok(())
    } else {
        Err(crate::tr!("error-winboat-not-required").into())
    }
}

#[tauri::command]
pub(crate) async fn preview_winboat_nvram(app: AppHandle) -> CommandResult<serde_json::Value> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    #[cfg(target_os = "linux")]
    {
        Ok(
            serde_json::to_value(crate::winboat::nvram::preview(&config).await?)
                .map_err(|_| crate::tr!("error-nvram-unsupported"))?,
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err(crate::tr!("error-winboat-not-required").into())
    }
}

#[tauri::command]
pub(crate) async fn recover_winboat_nvram(
    app: AppHandle,
    preview_id: String,
    confirmed: bool,
) -> CommandResult<serde_json::Value> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    #[cfg(target_os = "linux")]
    {
        Ok(serde_json::to_value(
            crate::winboat::nvram::recover(&config, &preview_id, confirmed).await?,
        )
        .map_err(|_| crate::tr!("error-nvram-unsupported"))?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, preview_id, confirmed);
        Err(crate::tr!("error-winboat-not-required").into())
    }
}

#[tauri::command]
pub(crate) async fn restore_winboat_nvram(
    app: AppHandle,
    preview_id: String,
    confirmed: bool,
) -> CommandResult<serde_json::Value> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    #[cfg(target_os = "linux")]
    {
        Ok(serde_json::to_value(
            crate::winboat::nvram::restore(&config, &preview_id, confirmed).await?,
        )
        .map_err(|_| crate::tr!("error-nvram-unsupported"))?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, preview_id, confirmed);
        Err(crate::tr!("error-winboat-not-required").into())
    }
}

#[cfg(test)]
mod tests {
    use super::get_capabilities;
    use crate::contracts::{BackendId, CapabilityId, PlatformId, CONTRACT_SCHEMA_VERSION};

    #[test]
    fn tauri_capability_command_returns_the_common_snapshot_contract() {
        let snapshot = get_capabilities(None).expect("current backend is available");
        assert_eq!(snapshot.schema_version, CONTRACT_SCHEMA_VERSION);
        assert_eq!(
            snapshot.manifest.capabilities.len(),
            CapabilityId::ALL.len()
        );
        assert_eq!(snapshot.manifest.host_platform, PlatformId::current());
        if cfg!(target_os = "linux") {
            assert_eq!(snapshot.manifest.backend, BackendId::LinuxWinboat);
            assert_eq!(snapshot.manifest.studio_platform, PlatformId::Windows);
        }
    }
}

#[cfg(test)]
#[path = "environment/tests.rs"]
mod setup_tests;

#[tauri::command]
pub(crate) fn preview_coordination_permissions(app: AppHandle) -> CommandResult<serde_json::Value> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    #[cfg(target_os = "linux")]
    {
        serde_json::to_value(crate::winboat::maintenance::repair::preview(&config)?).map_err(|_| {
            crate::winboat::maintenance::LockError::Unavailable
                .command_error(crate::contracts::CapabilityId::EnvironmentConfigure)
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err(crate::tr!("error-winboat-not-required").into())
    }
}

#[tauri::command]
pub(crate) fn repair_coordination_permissions(
    app: AppHandle,
    preview_id: String,
    confirmed: bool,
) -> CommandResult<()> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    #[cfg(target_os = "linux")]
    {
        crate::winboat::maintenance::repair::repair(&config, &preview_id, confirmed)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, preview_id, confirmed);
        Err(crate::tr!("error-winboat-not-required").into())
    }
}
