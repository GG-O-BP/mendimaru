mod reason;
mod report;
mod runner;

use super::container::ensure_private_operation_transport;
use super::remote_app::{spawn_powershell_file, RemoteAppProcess};
use super::security::OperationSecurity;
use crate::models::AppConfig;
use runner::wait_for_windows_operation;
use std::path::Path;
use std::time::Duration;

#[cfg(test)]
pub(super) use reason::localize_windows_reason;
#[cfg(test)]
pub(super) use report::parse_install_report;
pub(super) use report::{
    WindowsOperationReport, WindowsOperationState, WindowsStudioSessionReport,
};

#[derive(Debug)]
pub(crate) struct WindowsOperationFailure {
    pub(crate) message: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) retryable: bool,
}

pub(super) struct WindowsOperationOutcome {
    pub(super) report: WindowsOperationReport,
    pub(super) remote_app: Option<RemoteAppProcess>,
}

impl From<String> for WindowsOperationFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            exit_code: None,
            retryable: false,
        }
    }
}

const REMOTE_APP_START_ATTEMPTS: usize = 2;
const REMOTE_APP_RETRY_DELAY_SECONDS: u64 = 2;

pub(super) struct WindowsOperationRequest<'a> {
    pub(super) script_path: &'a Path,
    pub(super) script_sha256: &'a str,
    pub(super) label: &'a str,
    pub(super) report_path: &'a Path,
    pub(super) timeout_seconds: u64,
    pub(super) operation: &'a str,
    pub(super) keep_remote_app_alive: bool,
}

pub(super) async fn run_windows_operation<F>(
    config: &AppConfig,
    request: WindowsOperationRequest<'_>,
    mut on_report: F,
) -> Result<WindowsOperationOutcome, WindowsOperationFailure>
where
    F: FnMut(&WindowsOperationReport) + Send,
{
    ensure_private_operation_transport(config)?;
    for attempt in 0..REMOTE_APP_START_ATTEMPTS {
        remove_stale_report(request.report_path).await?;
        let security = OperationSecurity::generate(request.script_sha256)
            .map_err(|error| crate::tr!("error-operation-security-create", error = error))?;
        let mut remote_app =
            spawn_powershell_file(config, request.script_path, request.label, &security)?;
        match wait_for_windows_operation(
            request.report_path,
            &security,
            &mut remote_app,
            request.timeout_seconds,
            request.operation,
            &mut on_report,
        )
        .await
        {
            Ok(report) => {
                let remote_app = if request.keep_remote_app_alive {
                    Some(remote_app)
                } else {
                    stop_remote_app(&mut remote_app);
                    None
                };
                return Ok(WindowsOperationOutcome { report, remote_app });
            }
            Err(error) => {
                stop_remote_app(&mut remote_app);
                if error.retryable && attempt + 1 < REMOTE_APP_START_ATTEMPTS {
                    tokio::time::sleep(Duration::from_secs(REMOTE_APP_RETRY_DELAY_SECONDS)).await;
                    continue;
                }
                return Err(WindowsOperationFailure {
                    message: error.message,
                    exit_code: error.exit_code,
                    retryable: error.retryable || error.user_retryable,
                });
            }
        }
    }
    unreachable!("the RemoteApp attempt loop always returns")
}

async fn remove_stale_report(report_path: &Path) -> Result<(), String> {
    let mut temporary = report_path.as_os_str().to_os_string();
    temporary.push(".tmp");
    for path in [report_path.to_path_buf(), temporary.into()] {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(crate::tr!("error-operation-report-remove", error = error));
            }
        }
    }
    Ok(())
}

fn stop_remote_app(remote_app: &mut RemoteAppProcess) {
    match remote_app.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            let _ = remote_app.kill();
            let _ = remote_app.wait();
        }
    }
}
