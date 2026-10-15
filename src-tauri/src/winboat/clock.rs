#[cfg(target_os = "linux")]
use super::operation::{run_windows_operation, WindowsOperationRequest};
#[cfg(target_os = "linux")]
use super::studio::{secure_shared_directory, write_command_script};
use crate::app_paths::AppPaths;
use crate::contracts::{BackendError, BackendId, CapabilityId};
use crate::models::{AppConfig, CommandError, EnvironmentStatus};
#[cfg(target_os = "linux")]
use crate::models::{EnvironmentDiagnosticId, EnvironmentDiagnosticStatus};

#[cfg(target_os = "linux")]
pub(crate) const CLOSE_STUDIO: &str =
    "close all Studio Pro sessions before repairing the guest clock";
#[cfg(target_os = "linux")]
pub(crate) const REPAIR_FAILED: &str = "guest clock settings repair was not verified; changes may already be applied; check guest administrator permissions and docs/winboat-clock-sync.md";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClockRepairResult {
    status: EnvironmentStatus,
    clock_verified: bool,
    reboot_required: bool,
}

pub(crate) async fn repair(
    paths: &AppPaths,
    config: &AppConfig,
    rtc_base: &str,
) -> Result<ClockRepairResult, CommandError> {
    if !matches!(rtc_base, "utc" | "local") {
        return Err(BackendError::invalid_request("RTC base must be utc or local").into());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (paths, config);
        Err(BackendError::unsupported(
            if cfg!(target_os = "windows") {
                BackendId::WindowsNative
            } else {
                BackendId::MacNative
            },
            CapabilityId::EnvironmentConfigure,
        )
        .into())
    }
    #[cfg(target_os = "linux")]
    {
        let lease = super::vm_use::acquire(
            config,
            super::vm_use::Mode::Exclusive,
            CapabilityId::EnvironmentConfigure,
        )
        .await?;
        lease
            .run(async {
                if !super::registered_client_sessions().is_empty()
                    || !crate::cli::session_keepers_absent_for_launch(paths).await?
                {
                    return Err(precondition(CLOSE_STUDIO));
                }
                let directory = secure_shared_directory(config, ".mendimaru/operations")?;
                let mut random = [0_u8; 16];
                getrandom::fill(&mut random).map_err(|_| precondition(REPAIR_FAILED))?;
                let id = format!(
                    "clock-{}",
                    random
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                let report_path = directory.join(format!("{id}.json"));
                let windows_report_path = crate::projects::linux_path_to_windows_share(
                    std::path::Path::new(&config.shared_directory),
                    &report_path,
                    &config.windows_shared_directory,
                )?;
                let script = script(&windows_report_path, rtc_base);
                let command = write_command_script(config, &id, &script)?;
                let artifacts = Artifacts(vec![
                    command.path.clone(),
                    report_path.clone(),
                    report_path.with_extension("json.tmp"),
                ]);
                let outcome = run_windows_operation(
                    config,
                    WindowsOperationRequest {
                        script_path: &command.path,
                        script_sha256: &command.sha256,
                        label: "Mendimaru guest clock repair",
                        report_path: &report_path,
                        timeout_seconds: 90,
                        operation: "guest clock repair",
                        keep_remote_app_alive: false,
                        cancellation: None,
                        project_access: None,
                    },
                    |_| {},
                )
                .await;
                drop(artifacts);
                // Guest errors are never copied into public output. RTC/service changes
                // can already have succeeded when a later step fails, so do not claim rollback.
                outcome.map_err(|_| precondition(REPAIR_FAILED))?;
                let status = super::environment_status(config).await;
                let clock_verified = status.diagnostics.iter().any(|d| {
                    d.id == EnvironmentDiagnosticId::GuestClock
                        && d.status == EnvironmentDiagnosticStatus::Success
                });
                Ok(ClockRepairResult {
                    status,
                    clock_verified,
                    reboot_required: !clock_verified,
                })
            })
            .await
    }
}

#[cfg(target_os = "linux")]
fn precondition(message: &str) -> CommandError {
    let mut error = BackendError::operation(
        BackendId::LinuxWinboat,
        CapabilityId::EnvironmentConfigure,
        message,
    );
    error.code = crate::contracts::BackendErrorCode::PreconditionFailed;
    error.into()
}

#[cfg(target_os = "linux")]
struct Artifacts(Vec<std::path::PathBuf>);
#[cfg(target_os = "linux")]
impl Drop for Artifacts {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(any(test, target_os = "linux"))]
fn script(report_path: &str, rtc_base: &str) -> String {
    let script = r#"$ErrorActionPreference = 'Stop'
$resultPath = '__REPORT__'
__SECURITY__
__HELPER__
try {
    Repair-MendimaruGuestClock -RtcBase '__RTC__'
    Write-MendimaruReport @{ state='succeeded'; message='guest-clock-settings-repaired'; timestamp=[DateTime]::UtcNow.ToString('o') }
} catch {
    Write-MendimaruReport @{ state='failed'; message='guest-clock-sync-failed'; error='guest-clock-sync-failed'; timestamp=[DateTime]::UtcNow.ToString('o') }
}
"#;
    script
        .replace("__REPORT__", &report_path.replace('\'', "''"))
        .replace(
            "__SECURITY__",
            include_str!("../../scripts/operation_security.ps1"),
        )
        .replace("__HELPER__", include_str!("../../scripts/guest_clock.ps1"))
        .replace("__RTC__", rtc_base)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repair_is_authenticated_and_does_not_embed_host_time_that_can_go_stale() {
        let value = script(r"\\host.lan\Data\it's a report.json", "utc");
        assert!(!value.contains("__REPORT__"));
        assert!(!value.contains("__SECURITY__"));
        assert!(!value.contains("__HELPER__"));
        assert!(!value.contains("__RTC__"));
        assert!(value.contains("it's a report".replace('\'', "''").as_str()));
        assert!(value.contains("Repair-MendimaruGuestClock -RtcBase 'utc'"));
        assert!(value.contains("MENDIMARU_OPERATION_NONCE"));
        assert!(!value.contains("Set-Date"));
    }
}
