mod reason;
mod report;
mod runner;

use super::container::ensure_private_operation_transport;
use super::project_access::ProjectAccessLease;
use super::remote_app::{spawn_powershell_file, RemoteAppProcess};
use super::security::{AuthenticatedPayload, OperationSecurity};
use crate::contracts::{BackendError, BackendErrorCode};
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
    pub(crate) backend_error_code: Option<BackendErrorCode>,
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
            backend_error_code: None,
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
            backend_error_code: None,
        }
    }
}

impl From<BackendError> for WindowsOperationFailure {
    fn from(error: BackendError) -> Self {
        Self {
            message: error.message,
            exit_code: None,
            retryable: error.retryable,
            failure_kind: None,
            backend_error_code: Some(error.code),
        }
    }
}

const REMOTE_APP_START_ATTEMPTS: usize = 2;
const REMOTE_APP_RETRY_DELAY_SECONDS: u64 = 2;
/// Bound for the RDP endpoint to answer the X.224 negotiation after a
/// container recreation. The guest API is already online at this point, so
/// this only covers the RDP listener and the host port forwarder settling.
const REMOTE_DESKTOP_READY_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_DESKTOP_STABLE_PROBES: usize = 2;
const REMOTE_DESKTOP_PROBE_INTERVAL: Duration = Duration::from_millis(500);

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
    for attempt in 0..REMOTE_APP_START_ATTEMPTS {
        // Every container recreation can move the published RDP port inside
        // its host range (#227), so the endpoint is resolved per attempt.
        let connection_config = remote_desktop_connection(config).await?;
        wait_for_remote_desktop(&connection_config, REMOTE_DESKTOP_READY_TIMEOUT).await?;
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
                // The HMAC-authenticated report proves this connection reached
                // the real guest; only now may its certificate become trusted.
                remote_app.confirm_trust();
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
                remote_app.discard_unconfirmed_trust();
                if error.retryable && attempt + 1 < REMOTE_APP_START_ATTEMPTS {
                    tokio::time::sleep(Duration::from_secs(REMOTE_APP_RETRY_DELAY_SECONDS)).await;
                    continue;
                }
                return Err(wait_failure(error));
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
    .map_err(wait_failure)?;
    Ok((wait.report, wait.authenticated))
}

fn wait_failure(error: runner::WindowsOperationWaitError) -> WindowsOperationFailure {
    // A rejected RDP certificate is a trust decision, never a transient fault.
    let certificate_rejected =
        error.backend_error_code == Some(BackendErrorCode::RemoteDesktopCertificateMismatch);
    WindowsOperationFailure {
        message: error.message,
        exit_code: error.exit_code,
        retryable: !certificate_rejected && (error.retryable || error.user_retryable),
        failure_kind: error.failure_kind,
        backend_error_code: error.backend_error_code,
    }
}

async fn remote_desktop_connection(
    config: &AppConfig,
) -> Result<AppConfig, WindowsOperationFailure> {
    let mut connection_config = config.clone();
    connection_config.rdp_port = crate::config::runtime_host_port_async(config, 3389, "tcp")
        .await?
        .unwrap_or(config.rdp_port);
    Ok(connection_config)
}

async fn wait_for_remote_desktop(
    config: &AppConfig,
    timeout: Duration,
) -> Result<(), WindowsOperationFailure> {
    super::rdp_probe::wait_until_stable(
        &config.rdp_host,
        config.rdp_port,
        timeout,
        REMOTE_DESKTOP_STABLE_PROBES,
        REMOTE_DESKTOP_PROBE_INTERVAL,
    )
    .await
    .map_err(|_| remote_desktop_unavailable())
}

fn remote_desktop_unavailable() -> WindowsOperationFailure {
    WindowsOperationFailure {
        message: crate::tr!("error-remote-desktop-unavailable"),
        exit_code: None,
        retryable: true,
        failure_kind: Some(CommandFailureKind::Wait),
        backend_error_code: Some(BackendErrorCode::RemoteDesktopUnavailable),
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
    use super::{wait_failure, wait_for_remote_desktop};
    use crate::contracts::BackendErrorCode;
    use crate::models::{AppConfig, ContainerRuntime};
    use crate::process::CommandFailureKind;
    use crate::winboat::operation::runner::WindowsOperationWaitError;
    use crate::winboat::rdp_probe::tests::{fake_endpoint, Behaviour};
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
    async fn an_unready_remote_desktop_is_bounded_retryable_and_typed() {
        let error =
            wait_for_remote_desktop(&endpoint_config("127.0.0.1", 9), Duration::from_millis(10))
                .await
                .expect_err("the closed fixture endpoint is not ready");

        assert!(error.retryable);
        assert_eq!(error.failure_kind, Some(CommandFailureKind::Wait));
        assert_eq!(
            error.backend_error_code,
            Some(BackendErrorCode::RemoteDesktopUnavailable)
        );
    }

    #[tokio::test]
    async fn a_forwarder_that_accepts_tcp_is_not_a_ready_remote_desktop() {
        // A bare TCP connect succeeded here before #227 and FreeRDP was
        // started against a container that was still being recreated.
        let (port, _) = fake_endpoint(vec![Behaviour::CloseImmediately]).await;
        let error = wait_for_remote_desktop(
            &endpoint_config("127.0.0.1", port),
            Duration::from_millis(300),
        )
        .await
        .expect_err("accepting TCP is not answering RDP");
        assert_eq!(
            error.backend_error_code,
            Some(BackendErrorCode::RemoteDesktopUnavailable)
        );

        let (ready, _) = fake_endpoint(vec![Behaviour::Confirm]).await;
        wait_for_remote_desktop(&endpoint_config("127.0.0.1", ready), Duration::from_secs(5))
            .await
            .expect("a stable RDP endpoint is ready");
    }

    #[test]
    fn a_rejected_certificate_keeps_its_cause_and_is_never_retryable() {
        let failure = wait_failure(WindowsOperationWaitError {
            message: "rejected".into(),
            retryable: true,
            user_retryable: false,
            exit_code: None,
            failure_kind: None,
            backend_error_code: Some(BackendErrorCode::RemoteDesktopCertificateMismatch),
        });
        assert!(!failure.retryable);
        assert_eq!(
            failure.backend_error_code,
            Some(BackendErrorCode::RemoteDesktopCertificateMismatch)
        );

        let transient = wait_failure(WindowsOperationWaitError {
            message: "ended".into(),
            retryable: false,
            user_retryable: true,
            exit_code: None,
            failure_kind: Some(CommandFailureKind::Wait),
            backend_error_code: None,
        });
        assert!(transient.retryable);
        assert_eq!(transient.backend_error_code, None);
    }
}
