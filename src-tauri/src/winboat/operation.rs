mod reason;
mod report;
mod runner;

use super::remote_app::spawn_powershell_file;
use crate::models::AppConfig;
use runner::wait_for_windows_operation;
use std::path::Path;
use std::process::Child;
use std::time::Duration;

#[cfg(test)]
pub(super) use reason::localize_windows_reason;
#[cfg(test)]
pub(super) use report::parse_install_report;
pub(super) use report::{WindowsOperationReport, WindowsOperationState};

const REMOTE_APP_START_ATTEMPTS: usize = 2;
const REMOTE_APP_RETRY_DELAY_SECONDS: u64 = 2;

pub(super) struct WindowsOperationRequest<'a> {
    pub(super) script_path: &'a Path,
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
) -> Result<WindowsOperationReport, String>
where
    F: FnMut(&WindowsOperationReport) + Send,
{
    for attempt in 0..REMOTE_APP_START_ATTEMPTS {
        let mut remote_app = spawn_powershell_file(config, request.script_path, request.label)?;
        match wait_for_windows_operation(
            request.report_path,
            &mut remote_app,
            request.timeout_seconds,
            request.operation,
            &mut on_report,
        )
        .await
        {
            Ok(report) => {
                if !request.keep_remote_app_alive {
                    stop_remote_app(&mut remote_app);
                }
                return Ok(report);
            }
            Err(error) => {
                stop_remote_app(&mut remote_app);
                if error.retryable && attempt + 1 < REMOTE_APP_START_ATTEMPTS {
                    tokio::time::sleep(Duration::from_secs(REMOTE_APP_RETRY_DELAY_SECONDS)).await;
                    continue;
                }
                return Err(error.message);
            }
        }
    }
    unreachable!("the RemoteApp attempt loop always returns")
}

fn stop_remote_app(remote_app: &mut Child) {
    match remote_app.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            let _ = remote_app.kill();
            let _ = remote_app.wait();
        }
    }
}
