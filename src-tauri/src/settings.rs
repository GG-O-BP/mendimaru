use crate::models::{AppConfig, SettingsSaveResult};
use std::path::PathBuf;
use tauri::AppHandle;

pub async fn save_settings(
    app: &AppHandle,
    mut config: AppConfig,
    apply_mount: bool,
) -> Result<SettingsSaveResult, String> {
    crate::config::normalize_and_validate(&mut config)?;
    let previous_config = crate::config::load_config(app).ok();
    let config_snapshot = crate::config::snapshot_config(app)?;
    let compose_path = PathBuf::from(&config.compose_file);
    let compose_snapshot = compose_path
        .is_file()
        .then(|| crate::config::snapshot_file(&compose_path))
        .transpose()?;

    let mount_changed = if compose_path.is_file() {
        crate::config::update_shared_mount(&compose_path, &config.shared_directory)?
    } else {
        false
    };

    if let Err(error) = crate::config::persist_config(app, &config) {
        return Err(rollback_files(
            &config_snapshot,
            compose_snapshot.as_ref(),
            error,
        ));
    }

    let container_recreated = if mount_changed && apply_mount {
        if let Err(error) = crate::winboat::recreate_container(&config).await {
            let message = rollback_files(&config_snapshot, compose_snapshot.as_ref(), error);
            if let Some(previous) = previous_config {
                let _ = crate::winboat::recreate_container(&previous).await;
            }
            return Err(message);
        }
        true
    } else {
        false
    };

    Ok(SettingsSaveResult {
        config,
        mount_changed,
        container_recreated,
    })
}

fn rollback_files(
    config_snapshot: &crate::config::ConfigSnapshot,
    compose_snapshot: Option<&crate::config::FileSnapshot>,
    original_error: String,
) -> String {
    let mut rollback_errors = Vec::new();
    if let Err(error) = crate::config::restore_config(config_snapshot) {
        rollback_errors.push(error);
    }
    if let Some(snapshot) = compose_snapshot {
        if let Err(error) = crate::config::restore_file(snapshot) {
            rollback_errors.push(error);
        }
    }
    if rollback_errors.is_empty() {
        original_error
    } else {
        crate::tr!(
            "error-settings-rollback",
            error = original_error,
            rollback = rollback_errors.join("; ")
        )
    }
}
