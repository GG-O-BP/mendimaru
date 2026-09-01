mod reason;
mod report;
mod runner;

use super::container::ensure_private_operation_transport;
use super::project_access::ProjectAccessLease;
use super::remote_app::{spawn_powershell_file, RemoteAppProcess};
use super::security::{AuthenticatedPayload, OperationSecurity};
use crate::models::AppConfig;
use crate::process::{CancellationToken, CommandFailure, CommandFailureKind};
use runner::{
    wait_for_windows_operation, wait_for_windows_operation_after, WindowsOperationContinuation,
};
use std::path::Path;
use std::time::Duration;

#[cfg(test)]
pub(super) use reason::localize_windows_reason;
pub(super) use report::parse_install_report;
pub(super) use report::{
    WindowsOperationReport, WindowsOperationState, WindowsStudioSessionReport,
};

#[derive(Debug)]
pub(crate) struct WindowsOperationFailure {
    pub(crate) message: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) retryable: bool,
    pub(crate) failure_kind: Option<CommandFailureKind>,
}

pub(super) struct WindowsOperationOutcome {
    pub(super) report: WindowsOperationReport,
    pub(super) remote_app: Option<RemoteAppProcess>,
    pub(super) security: Option<OperationSecurity>,
    pub(super) authenticated: AuthenticatedPayload,
}

impl From<String> for WindowsOperationFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            exit_code: None,
            retryable: false,
            failure_kind: None,
        }
    }
}

impl From<CommandFailure> for WindowsOperationFailure {
    fn from(error: CommandFailure) -> Self {
        let failure_kind = Some(error.kind());
        Self {
            message: error.to_string(),
            exit_code: None,
            retryable: true,
            failure_kind,
        }
    }
}

const REMOTE_APP_START_ATTEMPTS: usize = 2;
const REMOTE_APP_RETRY_DELAY_SECONDS: u64 = 2;
const REMOTE_APP_ENDPOINT_READY_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct WindowsOperationRequest<'a> {
    pub(super) script_path: &'a Path,
    pub(super) script_sha256: &'a str,
    pub(super) label: &'a str,
    pub(super) report_path: &'a Path,
    pub(super) timeout_seconds: u64,
    pub(super) operation: &'a str,
    pub(super) keep_remote_app_alive: bool,
    pub(super) cancellation: Option<&'a CancellationToken>,
    pub(super) project_access: Option<&'a ProjectAccessLease>,
}

pub(super) async fn run_windows_operation<F>(
    config: &AppConfig,
    request: WindowsOperationRequest<'_>,
    mut on_report: F,
) -> Result<WindowsOperationOutcome, WindowsOperationFailure>
where
    F: FnMut(&WindowsOperationReport) + Send,
{
    ensure_private_operation_transport(config).await?;
    let mut connection_config = config.clone();
    connection_config.rdp_port = crate::config::runtime_host_port_async(config, 3389, "tcp")
        .await?
        .unwrap_or(config.rdp_port);
    wait_for_remote_app_endpoint(&connection_config, REMOTE_APP_ENDPOINT_READY_TIMEOUT).await?;
    for attempt in 0..REMOTE_APP_START_ATTEMPTS {
        remove_stale_report(request.report_path).await?;
        let security = OperationSecurity::generate(request.script_sha256)
            .map_err(|error| crate::tr!("error-operation-security-create", error = error))?;
        let spawn_config = connection_config.clone();
        let script_path = request.script_path.to_path_buf();
        let label = request.label.to_string();
        let spawn_security = security.clone();
        let project_access = request.project_access.cloned();
        let mut remote_app = tokio::task::spawn_blocking(move || {
            spawn_powershell_file(
                &spawn_config,
                &script_path,
                &label,
                &spawn_security,
                project_access.as_ref(),
            )
        })
        .await
        .map_err(|error| {
            WindowsOperationFailure::from(crate::tr!("error-native-process-join", error = error))
        })??;
        match wait_for_windows_operation(
            request.report_path,
            &security,
            &mut remote_app,
            request.timeout_seconds,
            request.operation,
            request.cancellation,
            &mut on_report,
        )
        .await
        {
            Ok(wait) => {
                let remote_app = if request.keep_remote_app_alive {
                    Some(remote_app)
                } else {
                    stop_remote_app(&mut remote_app);
                    None
                };
                let security = request.keep_remote_app_alive.then_some(security);
                return Ok(WindowsOperationOutcome {
                    report: wait.report,
                    remote_app,
                    security,
                    authenticated: wait.authenticated,
                });
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
                    failure_kind: error.failure_kind,
                });
            }
        }
    }
    unreachable!("the RemoteApp attempt loop always returns")
}

pub(super) async fn wait_for_followup_windows_operation(
    report_path: &Path,
    security: &OperationSecurity,
    previous_report: &AuthenticatedPayload,
    remote_app: &mut RemoteAppProcess,
    timeout_seconds: u64,
    operation: &str,
) -> Result<(WindowsOperationReport, AuthenticatedPayload), WindowsOperationFailure> {
    let wait = wait_for_windows_operation_after(
        report_path,
        security,
        remote_app,
        timeout_seconds,
        operation,
        WindowsOperationContinuation {
            previous_report: Some(previous_report),
            cancellation: None,
        },
        &mut |_| {},
    )
    .await
    .map_err(|error| WindowsOperationFailure {
        message: error.message,
        exit_code: error.exit_code,
        retryable: error.retryable || error.user_retryable,
        failure_kind: error.failure_kind,
    })?;
    Ok((wait.report, wait.authenticated))
}

async fn wait_for_remote_app_endpoint(
    config: &crate::models::AppConfig,
    timeout: Duration,
) -> Result<(), WindowsOperationFailure> {
    let started = tokio::time::Instant::now();
    loop {
        if tokio::net::TcpStream::connect((config.rdp_host.as_str(), config.rdp_port))
            .await
            .is_ok()
        {
            return Ok(());
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(WindowsOperationFailure {
                message: "the WinBoat RemoteApp endpoint was not ready before the deadline"
                    .to_string(),
                exit_code: None,
                retryable: true,
                failure_kind: Some(CommandFailureKind::Wait),
            });
        }
        let remaining = timeout - elapsed;
        tokio::time::sleep(remaining.min(Duration::from_millis(250))).await;
    }
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

#[cfg(test)]
mod tests {
    use super::wait_for_remote_app_endpoint;
    use crate::models::{AppConfig, ContainerRuntime};
    use crate::process::CommandFailureKind;
    use std::time::Duration;

    fn endpoint_config(host: &str, port: u16) -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "missing-winboat".into(),
            compose_file: "missing-compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:9".into(),
            rdp_host: host.into(),
            rdp_port: port,
            shared_directory: "/tmp/mendimaru-fixture".into(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "missing-freerdp".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        }
    }

    #[tokio::test]
    async fn an_unready_remoteapp_endpoint_is_bounded_and_retryable() {
        let error = wait_for_remote_app_endpoint(
            &endpoint_config("127.0.0.1", 9),
            Duration::from_millis(10),
        )
        .await
        .expect_err("the closed fixture endpoint is not ready");

        assert!(error.retryable);
        assert_eq!(error.failure_kind, Some(CommandFailureKind::Wait));
    }
}
