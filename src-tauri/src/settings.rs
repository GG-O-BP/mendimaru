use crate::config::{AppliedSharedMount, ComposeError, ComposeErrorKind};
use crate::models::{
    AppConfig, CommandError, CommandErrorCode, SettingsSavePreview, SettingsSaveResult,
};
use std::path::PathBuf;
use tauri::AppHandle;

pub fn preview_settings_save(
    mut config: AppConfig,
    apply_mount: bool,
) -> Result<Option<SettingsSavePreview>, CommandError> {
    crate::config::normalize_and_validate(&mut config)?;
    if crate::platform::is_windows_native() {
        return Ok(None);
    }

    let plan = crate::config::plan_shared_mount(
        &PathBuf::from(&config.compose_file),
        &config.shared_directory,
    )
    .map_err(compose_command_error)?;
    Ok(Some(SettingsSavePreview {
        service_name: plan.service_name().to_string(),
        current_shared_directory: plan.current_source().map(ToString::to_string),
        next_shared_directory: plan.next_source().to_string(),
        mount_changed: plan.mount_changed(),
        container_will_recreate: plan.mount_changed() && apply_mount,
        compose_revision: plan.revision().to_string(),
    }))
}

pub async fn save_settings(
    app: &AppHandle,
    mut config: AppConfig,
    apply_mount: bool,
    expected_compose_revision: Option<String>,
) -> Result<SettingsSaveResult, CommandError> {
    crate::config::normalize_and_validate(&mut config)?;
    if crate::platform::is_windows_native() {
        crate::config::persist_config(app, &config)?;
        return Ok(SettingsSaveResult {
            config,
            mount_changed: false,
            container_recreated: false,
        });
    }

    let previous_config = crate::config::load_config(app).ok();
    let config_snapshot = crate::config::snapshot_config(app)?;
    let plan = crate::config::plan_shared_mount(
        &PathBuf::from(&config.compose_file),
        &config.shared_directory,
    )
    .map_err(compose_command_error)?;
    let expected_revision = expected_compose_revision.as_deref().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::ComposeRevisionConflict,
            crate::tr!("error-compose-revision-conflict"),
        )
    })?;
    crate::config::verify_plan_revision(&plan, expected_revision).map_err(compose_command_error)?;
    plan.apply_detection(&mut config);

    let applied = crate::config::apply_shared_mount(plan).map_err(compose_command_error)?;
    let mount_changed = applied.changed;

    if let Err(error) = crate::config::persist_config(app, &config) {
        return Err(rollback_files(&config_snapshot, &applied, error).0.into());
    }

    let container_recreated = if mount_changed && apply_mount {
        if let Err(error) =
            crate::winboat::recreate_compose_service(&config, &applied.service_name).await
        {
            let (message, compose_restored) = rollback_files(&config_snapshot, &applied, error);
            if compose_restored {
                if let Some(previous) = previous_config {
                    let _ = crate::winboat::recreate_container(&previous).await;
                }
            }
            return Err(message.into());
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

fn compose_command_error(error: ComposeError) -> CommandError {
    let code = match error.kind() {
        ComposeErrorKind::NotWinboat => CommandErrorCode::ComposeNotWinboat,
        ComposeErrorKind::Ambiguous => CommandErrorCode::ComposeAmbiguous,
        ComposeErrorKind::RevisionConflict => CommandErrorCode::ComposeRevisionConflict,
        ComposeErrorKind::Other => CommandErrorCode::OperationFailed,
    };
    CommandError::new(code, error.to_string())
}

fn rollback_files(
    config_snapshot: &crate::config::ConfigSnapshot,
    applied: &AppliedSharedMount,
    original_error: String,
) -> (String, bool) {
    let mut rollback_errors = Vec::new();
    if let Err(error) = crate::config::restore_config(config_snapshot) {
        rollback_errors.push(error);
    }
    let compose_restored = if applied.changed {
        match crate::config::restore_file_if_revision(&applied.original, &applied.applied_revision)
        {
            Ok(()) => true,
            Err(error) => {
                rollback_errors.push(error);
                false
            }
        }
    } else {
        true
    };
    let message = if rollback_errors.is_empty() {
        original_error
    } else {
        crate::tr!(
            "error-settings-rollback",
            error = original_error,
            rollback = rollback_errors.join("; ")
        )
    };
    (message, compose_restored)
}

#[cfg(test)]
mod tests {
    use super::compose_command_error;
    use crate::models::CommandErrorCode;
    use std::fs;

    #[test]
    fn compose_identity_and_revision_failures_keep_distinct_command_codes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let compose = temporary.path().join("compose.yml");
        fs::write(
            &compose,
            "services:\n  windows:\n    image: nginx:latest\n    volumes:\n      - data:/storage\n",
        )
        .expect("ordinary Compose");
        let not_winboat = crate::config::plan_shared_mount(&compose, "/new")
            .expect_err("ordinary Compose rejected");
        assert_eq!(
            compose_command_error(not_winboat).code,
            CommandErrorCode::ComposeNotWinboat
        );

        fs::write(
            &compose,
            "services:\n  one:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes: [one:/storage]\n  two:\n    image: ghcr.io/dockur/windows:6.03\n    labels: [winboat=true]\n    volumes: [two:/storage]\n",
        )
        .expect("ambiguous Compose");
        let ambiguous = crate::config::plan_shared_mount(&compose, "/new")
            .expect_err("ambiguous Compose rejected");
        assert_eq!(
            compose_command_error(ambiguous).code,
            CommandErrorCode::ComposeAmbiguous
        );

        fs::write(
            &compose,
            "services:\n  vm:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - /old:/shared\n      - data:/storage\n",
        )
        .expect("WinBoat Compose");
        let plan = crate::config::plan_shared_mount(&compose, "/new").expect("mount plan");
        fs::write(&compose, "external edit\n").expect("external edit");
        let conflict = crate::config::apply_shared_mount(plan).expect_err("conflict rejected");
        assert_eq!(
            compose_command_error(conflict).code,
            CommandErrorCode::ComposeRevisionConflict
        );
    }
}
