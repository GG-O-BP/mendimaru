use super::reason::{localize_operation_state, localize_windows_reason};
use super::report::{parse_install_report, WindowsOperationReport, WindowsOperationState};
use crate::process::{CancellationToken, CommandFailureKind};
use crate::winboat::remote_app::RemoteAppProcess;
use crate::winboat::security::{
    authenticate_report, OperationSecurity, ReportSequenceTracker, MAX_REPORT_BYTES,
};
use std::path::Path;
use std::time::Duration;

const REMOTE_APP_START_GRACE_SECONDS: u64 = 20;
const INSTALL_REPORT_STALE_SECONDS: u64 = 30;

pub(super) struct WindowsOperationWaitOutcome {
    pub(super) report: WindowsOperationReport,
    pub(super) authenticated: crate::winboat::security::AuthenticatedPayload,
}

pub(super) struct WindowsOperationWaitError {
    pub(super) message: String,
    pub(super) retryable: bool,
    pub(super) user_retryable: bool,
    pub(super) exit_code: Option<i32>,
    pub(super) failure_kind: Option<CommandFailureKind>,
}

pub(super) struct WindowsOperationContinuation<'a> {
    pub(super) previous_report: Option<&'a crate::winboat::security::AuthenticatedPayload>,
    pub(super) cancellation: Option<&'a CancellationToken>,
}

pub(super) async fn wait_for_windows_operation<F>(
    report_path: &Path,
    security: &OperationSecurity,
    remote_app: &mut RemoteAppProcess,
    timeout_seconds: u64,
    operation: &str,
    cancellation: Option<&CancellationToken>,
    on_report: &mut F,
) -> Result<WindowsOperationWaitOutcome, WindowsOperationWaitError>
where
    F: FnMut(&WindowsOperationReport) + Send,
{
    wait_for_windows_operation_after(
        report_path,
        security,
        remote_app,
        timeout_seconds,
        operation,
        WindowsOperationContinuation {
            previous_report: None,
            cancellation,
        },
        on_report,
    )
    .await
}

pub(super) async fn wait_for_windows_operation_after<F>(
    report_path: &Path,
    security: &OperationSecurity,
    remote_app: &mut RemoteAppProcess,
    timeout_seconds: u64,
    operation: &str,
    continuation: WindowsOperationContinuation<'_>,
    on_report: &mut F,
) -> Result<WindowsOperationWaitOutcome, WindowsOperationWaitError>
where
    F: FnMut(&WindowsOperationReport) + Send,
{
    let started = tokio::time::Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
    let mut remote_app_exited_at = None;
    let mut last_report_state = None;
    let mut last_progress_signature: Option<(WindowsOperationState, Option<i32>, bool)> = None;
    let mut last_report_timestamp = None;
    let mut last_report_changed_at = None;
    let mut last_report_had_percentage = false;
    let mut report_sequence = continuation
        .previous_report
        .map(ReportSequenceTracker::after)
        .unwrap_or_default();

    loop {
        if continuation
            .cancellation
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(WindowsOperationWaitError {
                message: "the Windows operation was cancelled".to_string(),
                retryable: false,
                user_retryable: true,
                exit_code: None,
                failure_kind: Some(CommandFailureKind::Cancelled),
            });
        }
        if remote_app.certificate_failed() {
            return Err(WindowsOperationWaitError {
                message: crate::tr!("error-freerdp-certificate-mismatch"),
                retryable: false,
                user_retryable: false,
                exit_code: None,
                failure_kind: None,
            });
        }
        match tokio::fs::symlink_metadata(report_path).await {
            Ok(metadata) => {
                if !is_bounded_regular_report(&metadata) {
                    return Err(report_authentication_error(
                        operation,
                        "the report path is not a bounded regular file",
                    ));
                }
                let content = tokio::fs::read(report_path)
                    .await
                    .map_err(|error| report_authentication_error(operation, &error.to_string()))?;
                let authenticated = authenticate_report(&content, security)
                    .map_err(|error| report_authentication_error(operation, &error.to_string()))?;
                let is_new = report_sequence
                    .accept(&authenticated)
                    .map_err(|error| report_authentication_error(operation, &error.to_string()))?;
                if is_new {
                    let report = parse_install_report(&authenticated.payload).map_err(|error| {
                        report_authentication_error(operation, &error.to_string())
                    })?;
                    last_report_state = Some(report.state);
                    last_report_had_percentage = report.percentage.is_some();
                    if last_report_timestamp.as_ref() != Some(&report.timestamp) {
                        last_report_timestamp = Some(report.timestamp.clone());
                        last_report_changed_at = Some(tokio::time::Instant::now());
                    }
                    let progress_signature = (
                        report.state,
                        report.percentage.map(|value| (value * 10.0).round() as i32),
                        report.estimated,
                    );
                    if last_progress_signature.as_ref() != Some(&progress_signature) {
                        on_report(&report);
                        last_progress_signature = Some(progress_signature);
                    }
                    match report.state {
                        WindowsOperationState::Succeeded => {
                            return Ok(WindowsOperationWaitOutcome {
                                report,
                                authenticated,
                            })
                        }
                        WindowsOperationState::Failed => {
                            return Err(failed_operation(&report, operation));
                        }
                        _ => {}
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(report_authentication_error(operation, &error.to_string()));
            }
        }

        if remote_app_exited_at.is_none() {
            match remote_app.try_wait() {
                Ok(Some(status)) => {
                    remote_app_exited_at = Some((tokio::time::Instant::now(), status))
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(WindowsOperationWaitError {
                        message: crate::tr!(
                            "error-remoteapp-state",
                            operation = operation,
                            error = error
                        ),
                        retryable: false,
                        user_retryable: false,
                        exit_code: None,
                        failure_kind: None,
                    });
                }
            }
        }

        if started.elapsed() >= timeout {
            return Err(operation_timeout_error(timeout_seconds, operation));
        }

        if let Some((exited_at, status)) = remote_app_exited_at {
            if exited_at.elapsed() >= Duration::from_secs(REMOTE_APP_START_GRACE_SECONDS) {
                let report_is_live = last_report_had_percentage
                    && last_report_changed_at.is_some_and(|changed_at| {
                        changed_at.elapsed() < Duration::from_secs(INSTALL_REPORT_STALE_SECONDS)
                    });
                if report_is_live {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                return match last_report_state {
                    Some(state) => Err(remote_app_ended_after_progress(state, status, operation)),
                    None => Err(remote_app_ended_before_progress(status, operation)),
                };
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn operation_timeout_error(timeout_seconds: u64, operation: &str) -> WindowsOperationWaitError {
    WindowsOperationWaitError {
        message: crate::tr!(
            "error-operation-timeout",
            operation = operation,
            minutes = crate::i18n::format_number(timeout_seconds / 60)
        ),
        retryable: true,
        user_retryable: true,
        exit_code: None,
        failure_kind: Some(CommandFailureKind::Timeout),
    }
}

fn remote_app_ended_after_progress(
    state: WindowsOperationState,
    status: std::process::ExitStatus,
    operation: &str,
) -> WindowsOperationWaitError {
    WindowsOperationWaitError {
        message: crate::tr!(
            "error-remoteapp-ended",
            operation = operation,
            state = localize_operation_state(state),
            status = status
        ),
        retryable: true,
        user_retryable: true,
        exit_code: None,
        failure_kind: Some(CommandFailureKind::Wait),
    }
}

fn remote_app_ended_before_progress(
    status: std::process::ExitStatus,
    operation: &str,
) -> WindowsOperationWaitError {
    WindowsOperationWaitError {
        message: crate::tr!(
            "error-operation-not-started",
            operation = operation,
            status = status
        ),
        retryable: true,
        user_retryable: true,
        exit_code: None,
        failure_kind: Some(CommandFailureKind::Wait),
    }
}

fn is_bounded_regular_report(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_file() && metadata.len() <= MAX_REPORT_BYTES
}

fn report_authentication_error(operation: &str, reason: &str) -> WindowsOperationWaitError {
    WindowsOperationWaitError {
        message: crate::tr!(
            "error-operation-report-authentication",
            operation = operation,
            reason = reason
        ),
        retryable: false,
        user_retryable: false,
        exit_code: None,
        failure_kind: None,
    }
}

fn failed_operation(report: &WindowsOperationReport, operation: &str) -> WindowsOperationWaitError {
    let raw_reason = report
        .error
        .as_deref()
        .filter(|message| !message.is_empty())
        .unwrap_or(&report.message);
    let reason = localize_windows_reason(raw_reason);
    let message = if let Some(code) = report.exit_code {
        let code = if code >= 0 {
            crate::i18n::format_number(code as u64)
        } else {
            code.to_string()
        };
        crate::tr!(
            "error-windows-operation-code",
            operation = operation,
            code = &code,
            reason = &reason
        )
    } else {
        crate::tr!(
            "error-windows-operation",
            operation = operation,
            reason = &reason
        )
    };
    WindowsOperationWaitError {
        message,
        retryable: false,
        user_retryable: true,
        exit_code: report.exit_code,
        failure_kind: None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        failed_operation, is_bounded_regular_report, operation_timeout_error,
        remote_app_ended_after_progress, remote_app_ended_before_progress,
    };
    use crate::winboat::operation::{WindowsOperationReport, WindowsOperationState};
    use std::os::unix::fs::symlink;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn refuses_symlinked_and_oversized_operation_reports() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("target.json");
        let link = directory.path().join("report.json");
        std::fs::write(&target, b"{}").expect("write target");
        symlink(&target, &link).expect("create report symlink");
        assert!(!is_bounded_regular_report(
            &std::fs::symlink_metadata(&link).expect("read symlink metadata")
        ));

        let oversized = directory.path().join("oversized.json");
        let file = std::fs::File::create(&oversized).expect("create oversized report");
        file.set_len(super::MAX_REPORT_BYTES + 1)
            .expect("extend report");
        assert!(!is_bounded_regular_report(
            &std::fs::symlink_metadata(&oversized).expect("read oversized metadata")
        ));
    }

    #[test]
    fn preserves_an_authenticated_windows_exit_code_as_structured_failure_data() {
        let report = WindowsOperationReport {
            state: WindowsOperationState::Failed,
            message: "installer failed".into(),
            percentage: Some(75.0),
            estimated: false,
            timestamp: "2026-08-15T00:00:00Z".into(),
            exit_code: Some(1603),
            executable_path: None,
            error: Some("InstallerExitCode".into()),
            sessions: Vec::new(),
        };

        let error = failed_operation(&report, "installing Studio Pro");
        assert_eq!(error.exit_code, Some(1603));
        assert!(!error.retryable);
        assert!(error.user_retryable);
    }

    #[test]
    fn transient_remoteapp_startup_failures_are_retryable_and_classified() {
        let timeout = operation_timeout_error(90, "starting Studio Pro");
        assert!(timeout.retryable);
        assert!(timeout.user_retryable);
        assert_eq!(
            timeout.failure_kind,
            Some(crate::process::CommandFailureKind::Timeout)
        );

        let before_progress = remote_app_ended_before_progress(
            std::process::ExitStatus::from_raw(1),
            "starting Studio Pro",
        );
        let after_progress = remote_app_ended_after_progress(
            WindowsOperationState::Running,
            std::process::ExitStatus::from_raw(1),
            "starting Studio Pro",
        );
        for error in [before_progress, after_progress] {
            assert!(error.retryable);
            assert!(error.user_retryable);
            assert_eq!(
                error.failure_kind,
                Some(crate::process::CommandFailureKind::Wait)
            );
        }
    }
}
