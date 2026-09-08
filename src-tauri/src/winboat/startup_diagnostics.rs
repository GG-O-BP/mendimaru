//! Only allowlisted signatures leave this module; raw container output never does.
use crate::contracts::BackendErrorCode;
use crate::models::{
    AppConfig, EnvironmentDiagnostic, EnvironmentDiagnosticAction, EnvironmentDiagnosticErrorCode,
    EnvironmentDiagnosticId, EnvironmentDiagnosticStatus,
};
use crate::process::{self, CommandPolicy};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::Instant;

pub(super) const LOG_BYTES: usize = 16 * 1024;
const LOG_LINES: usize = 100;
const QEMU_TIMEOUT: &str = "ERROR: Timeout while waiting for QEMU to boot the machine!";

fn classify(bytes: &[u8]) -> Option<BackendErrorCode> {
    let bounded = &bytes[..bytes.len().min(LOG_BYTES)];
    let text = String::from_utf8_lossy(bounded);
    text.lines().take(LOG_LINES).find_map(|line| {
        let line = line.trim();
        // Dockur's logger may wrap the fixed error line in red SGR/reset.
        let line = line.strip_prefix("\u{1b}[1;31m").unwrap_or(line);
        let line = line.strip_suffix("\u{1b}[0m").unwrap_or(line).trim();
        let line = line.strip_prefix("❯ ").unwrap_or(line);
        (line == QEMU_TIMEOUT).then_some(BackendErrorCode::QemuBootTimeout)
    })
}

pub(super) async fn classify_exit(
    config: &AppConfig,
    started_at: &str,
    deadline: Instant,
) -> BackendErrorCode {
    let fallback = BackendErrorCode::ContainerExitedDuringStartup;
    let Some(policy) = deadline
        .checked_duration_since(Instant::now())
        .and_then(|d| CommandPolicy::within_budget(d, Duration::from_secs(2), LOG_BYTES))
    else {
        return fallback;
    };
    let mut command = Command::new(config.container_runtime.as_str());
    command.args([
        "logs",
        "--since",
        started_at,
        "--tail",
        "100",
        &config.container_name,
    ]);
    let Ok(output) = process::output(command, policy, None, "startup diagnostics").await else {
        return fallback;
    };
    if !output.status.success() {
        return fallback;
    }
    classify(&output.stdout)
        .or_else(|| classify(&output.stderr))
        .unwrap_or(fallback)
}

pub(super) fn apply(
    attempt: Option<&super::startup::StartupAttempt>,
    diagnostics: &mut [EnvironmentDiagnostic],
) {
    let Some(attempt) = attempt.filter(|a| a.phase == super::startup::StartupPhase::StartupFailed)
    else {
        return;
    };
    let code = match attempt.error_code {
        Some(BackendErrorCode::QemuBootTimeout) => EnvironmentDiagnosticErrorCode::QemuBootTimeout,
        Some(BackendErrorCode::ContainerExitedDuringStartup) => {
            EnvironmentDiagnosticErrorCode::ContainerExitedDuringStartup
        }
        Some(BackendErrorCode::GuestStartupTimeout) => {
            EnvironmentDiagnosticErrorCode::GuestStartupTimeout
        }
        Some(BackendErrorCode::ExternalProcessTimeout) => {
            EnvironmentDiagnosticErrorCode::ExternalProcessTimeout
        }
        _ => return,
    };
    if let Some(check) = diagnostics
        .iter_mut()
        .find(|d| d.id == EnvironmentDiagnosticId::Container)
    {
        check.status = EnvironmentDiagnosticStatus::Failure;
        check.error_code = Some(code);
        check.action = Some(EnvironmentDiagnosticAction::OpenWinboat);
        check.observed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_allowlisted_signatures_produce_a_specific_code() {
        for line in [
            QEMU_TIMEOUT.to_string(),
            format!("❯ {QEMU_TIMEOUT}"),
            format!("\u{1b}[1;31m❯ {QEMU_TIMEOUT}\u{1b}[0m"),
            format!("{QEMU_TIMEOUT}\npassword=private token=secret /home/private"),
        ] {
            assert_eq!(
                classify(line.as_bytes()),
                Some(BackendErrorCode::QemuBootTimeout)
            );
        }
        for line in [
            "password=private token=secret /home/private".to_string(),
            format!("untrusted prefix {QEMU_TIMEOUT}"),
            format!("{QEMU_TIMEOUT} password=secret"),
        ] {
            assert_eq!(classify(line.as_bytes()), None);
        }
    }

    #[test]
    fn logs_beyond_byte_and_line_limits_are_not_classified() {
        assert_eq!(
            classify(format!("{}\n{QEMU_TIMEOUT}", "x".repeat(LOG_BYTES)).as_bytes()),
            None
        );
        assert_eq!(
            classify(format!("{}{QEMU_TIMEOUT}", "unknown\n".repeat(LOG_LINES)).as_bytes()),
            None
        );
        assert_eq!(classify(&[0xff; LOG_BYTES + 1]), None);
    }

    #[test]
    fn diagnostics_contain_only_codes_and_recovery_actions() {
        let attempt = super::super::startup::StartupAttempt {
            id: 7,
            started_at: "2026-09-08T00:00:00Z".into(),
            phase: super::super::startup::StartupPhase::StartupFailed,
            error_code: Some(BackendErrorCode::QemuBootTimeout),
            container_status: crate::models::ContainerStatus::Exited,
        };
        let mut checks = vec![EnvironmentDiagnostic {
            id: EnvironmentDiagnosticId::Container,
            status: EnvironmentDiagnosticStatus::Success,
            observed: Some("password=secret /home/private".into()),
            action: None,
            error_code: None,
        }];
        apply(Some(&attempt), &mut checks);
        let json = serde_json::to_string(&checks).unwrap();
        assert!(json.contains("qemu-boot-timeout"));
        assert!(json.contains("open-winboat"));
        for secret in ["password", "secret", "/home/private", QEMU_TIMEOUT] {
            assert!(!json.contains(secret));
        }
    }
}
