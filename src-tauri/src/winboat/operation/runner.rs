use super::reason::{localize_operation_state, localize_windows_reason};
use super::report::{parse_install_report, WindowsOperationReport, WindowsOperationState};
use std::path::Path;
use std::process::Child;
use std::time::Duration;

const REMOTE_APP_START_GRACE_SECONDS: u64 = 20;
const INSTALL_REPORT_STALE_SECONDS: u64 = 30;

pub(super) struct WindowsOperationWaitError {
    pub(super) message: String,
    pub(super) retryable: bool,
}

pub(super) async fn wait_for_windows_operation<F>(
    report_path: &Path,
    remote_app: &mut Child,
    timeout_seconds: u64,
    operation: &str,
    on_report: &mut F,
) -> Result<WindowsOperationReport, WindowsOperationWaitError>
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

    loop {
        if let Ok(content) = tokio::fs::read_to_string(report_path).await {
            if let Ok(report) = parse_install_report(&content) {
                last_report_state = Some(report.state);
                last_report_had_percentage = report.percentage.is_some();
                if last_report_timestamp != report.timestamp {
                    last_report_timestamp = report.timestamp.clone();
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
                    WindowsOperationState::Succeeded => return Ok(report),
                    WindowsOperationState::Failed => {
                        return Err(failed_operation(&report, operation));
                    }
                    _ => {}
                }
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
                    });
                }
            }
        }

        if started.elapsed() >= timeout {
            return Err(WindowsOperationWaitError {
                message: crate::tr!(
                    "error-operation-timeout",
                    operation = operation,
                    minutes = crate::i18n::format_number(timeout_seconds / 60)
                ),
                retryable: false,
            });
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
                    Some(state) => Err(WindowsOperationWaitError {
                        message: crate::tr!(
                            "error-remoteapp-ended",
                            operation = operation,
                            state = localize_operation_state(state),
                            status = status
                        ),
                        retryable: false,
                    }),
                    None => Err(WindowsOperationWaitError {
                        message: crate::tr!(
                            "error-operation-not-started",
                            operation = operation,
                            status = status
                        ),
                        retryable: true,
                    }),
                };
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
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
    }
}
