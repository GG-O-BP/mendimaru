use super::{load_command_config, CommandResult};
use crate::browser::frontend::FrontendHealth;
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn diagnose_frontend_health(
    app: AppHandle,
    target: String,
) -> CommandResult<FrontendHealth> {
    let manifest = crate::platform::capability_manifest(None)?;
    let config = if target.starts_with("runtime_") {
        Some(load_command_config(&app)?)
    } else {
        None
    };
    crate::application::browser_frontend_health(
        config.as_ref(),
        manifest.backend,
        &target,
        15_000,
        3_000,
    )
    .await
}
