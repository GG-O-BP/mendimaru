use super::{load_command_config, CommandResult};
use crate::contracts::{BackendId, CapabilitySnapshot};
use crate::models::{EnvironmentStatus, SettingsSaveResult};
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn get_capabilities(backend: Option<BackendId>) -> CommandResult<CapabilitySnapshot> {
    Ok(crate::platform::capability_snapshot(backend)?)
}

#[tauri::command]
pub(crate) async fn get_environment_status(app: AppHandle) -> CommandResult<EnvironmentStatus> {
    let config = load_command_config(&app)?;
    Ok(crate::platform::environment_status(&config).await)
}

#[tauri::command]
pub(crate) async fn start_winboat_windows(app: AppHandle) -> CommandResult<()> {
    require_winboat()?;
    let config = load_command_config(&app)?;
    crate::winboat::start_container(&config).await?;
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

fn require_winboat() -> CommandResult<()> {
    if crate::platform::capabilities().requires_winboat {
        Ok(())
    } else {
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
