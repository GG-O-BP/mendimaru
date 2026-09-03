use super::client::installed_versions_cached;
use super::container::ensure_guest_online;
use super::operation::{
    run_windows_operation, WindowsOperationFailure, WindowsOperationRequest, WindowsOperationState,
    WindowsStudioSessionReport,
};
use super::scripts::{
    abort_studio_launch_script, install_script, launch_studio_script, uninstall_script,
};
use crate::models::{AppConfig, StudioInstallPhase, StudioInstallProgress};
use crate::platform::validate_version;
use crate::process::CancellationToken;
use crate::projects::{linux_path_to_windows_share, validate_project_selection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const STUDIO_LAUNCH_TIMEOUT_SECONDS: u64 = 5 * 60;
const STUDIO_LAUNCH_ATTEMPTS: usize = 2;
const STUDIO_LAUNCH_RETRY_DELAY_SECONDS: u64 = 2;
const FAILED_LAUNCH_ABORT_TIMEOUT_SECONDS: u64 = 90;
const INSTALL_TIMEOUT_SECONDS: u64 = 45 * 60;
const UNINSTALL_TIMEOUT_SECONDS: u64 = 15 * 60;
const WINDOWS_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionObservation {
    Absent,
    Present,
    Unverified,
}

#[derive(Debug)]
struct PostSuccessCleanup {
    remote_app_cleanup: &'static str,
    studio_stop: &'static str,
    runtime_stop: &'static str,
    runtime_stop_code: Option<crate::contracts::BackendErrorCode>,
    session_observation: SessionObservation,
    lock_cleanup: &'static str,
    locks_removed: usize,
    report_written: bool,
}

impl PostSuccessCleanup {
    fn retryable(&self) -> bool {
        matches!(self.remote_app_cleanup, "succeeded" | "not_owned")
            && self.studio_stop != "failed"
            && self.runtime_stop == "succeeded"
            && self.session_observation == SessionObservation::Absent
            && self.lock_cleanup != "failed"
            && self.report_written
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostSuccessCleanupReport<'a> {
    schema_version: &'a str,
    operation_id: &'a str,
    studio_session_id: &'a str,
    runtime_session_id: &'a str,
    remote_app_cleanup: &'static str,
    studio_stop: &'static str,
    runtime_stop: &'static str,
    runtime_stop_code: Option<crate::contracts::BackendErrorCode>,
    session_observation: &'static str,
    lock_cleanup: &'static str,
    locks_removed: usize,
    report_written: bool,
    retryable: bool,
}

pub async fn launch_studio(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
    project_mpr_path: Option<&str>,
) -> Result<(), WindowsOperationFailure> {
    validate_operation_id(operation_id)?;
    ensure_no_registered_remote_app()?;
    ensure_guest_online(config).await?;
    let versions = installed_versions_cached(config).await?;
    let selected = versions
        .into_iter()
        .find(|installed| installed.version == version)
        .ok_or_else(|| crate::tr!("error-studio-install-not-found", version = version))?;

    let project_access = project_mpr_path
        .map(|project_path| {
            let selection = validate_project_selection(config, Path::new(project_path))?;
            super::project_access::prepare(config, &selection)
        })
        .transpose()?;
    let project_argument = project_access
        .as_ref()
        .map(|lease| lease.guest_project_path());
    let project_directory = project_access
        .as_ref()
        .map(|lease| lease.host_project_directory().to_path_buf());
    let label = format!("Studio Pro {}", selected.version);
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let control_path = operation_directory.join(format!("{operation_id}.control.json"));
    ensure_control_path_available(&control_path)?;
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let windows_control_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &control_path,
        &config.windows_shared_directory,
    )?;
    let script = launch_studio_script(
        &selected.executable_path,
        project_argument,
        &windows_report_path,
        &windows_control_path,
        &config.mendix_install_root,
        version,
        project_access
            .as_ref()
            .map_or(0, |lease| lease.readiness_timeout_seconds()),
    );
    let command = write_command_script(config, operation_id, &script)?;
    let operation = crate::tr!("operation-studio-launch");
    let runtime_session_id = prepare_studio_runtime_session(config, project_mpr_path).await?;
    let mut completed = None;
    for attempt in 0..STUDIO_LAUNCH_ATTEMPTS {
        let mut launched_session = None;
        let result = run_windows_operation(
            config,
            WindowsOperationRequest {
                script_path: &command.path,
                script_sha256: &command.sha256,
                label: &label,
                report_path: &report_path,
                timeout_seconds: STUDIO_LAUNCH_TIMEOUT_SECONDS,
                operation: &operation,
                keep_remote_app_alive: true,
                cancellation: None,
                project_access: project_access.as_ref(),
            },
            |report| {
                if report.state == WindowsOperationState::Running && report.sessions.len() == 1 {
                    launched_session = report.sessions.first().cloned();
                }
            },
        )
        .await;
        match result {
            Ok(outcome) => {
                completed = Some(outcome);
                break;
            }
            Err(mut error) => {
                let Some(session) = launched_session else {
                    stop_runtime_session(config, &runtime_session_id).await;
                    return Err(error);
                };
                match abort_incomplete_launch(config, &selected, &session, operation_id, attempt)
                    .await
                {
                    Ok(()) if attempt + 1 < STUDIO_LAUNCH_ATTEMPTS => {
                        tokio::time::sleep(Duration::from_secs(STUDIO_LAUNCH_RETRY_DELAY_SECONDS))
                            .await;
                        continue;
                    }
                    Ok(()) => {
                        error.retryable = true;
                        stop_runtime_session(config, &runtime_session_id).await;
                        return Err(error);
                    }
                    Err(abort_error) => {
                        stop_runtime_session(config, &runtime_session_id).await;
                        error.message = format!(
                            "{} Incomplete launch cleanup also failed: {}",
                            error.message, abort_error.message
                        );
                        error.retryable = false;
                        return Err(error);
                    }
                }
            }
        }
    }
    let mut outcome = completed.ok_or_else(|| {
        WindowsOperationFailure::from("Studio Pro launch attempts were exhausted".to_string())
    })?;
    if outcome
        .report
        .executable_path
        .as_deref()
        .is_none_or(str::is_empty)
    {
        let session_id = outcome
            .report
            .sessions
            .first()
            .map(|session| session.session_id.clone());
        let recovery = recover_post_success_failure(
            config,
            &operation_directory,
            operation_id,
            &runtime_session_id,
            session_id.as_deref(),
            project_directory.as_deref(),
            outcome.remote_app.take(),
        )
        .await;
        let mut error = WindowsOperationFailure::from(crate::tr!("error-launch-path-missing"));
        error.retryable = recovery.retryable();
        return Err(error);
    }
    let client = match outcome.remote_app.take() {
        Some(client) => client,
        None => {
            let session_id = outcome
                .report
                .sessions
                .first()
                .map(|session| session.session_id.clone());
            let recovery = recover_post_success_failure(
                config,
                &operation_directory,
                operation_id,
                &runtime_session_id,
                session_id.as_deref(),
                project_directory.as_deref(),
                None,
            )
            .await;
            let mut error = WindowsOperationFailure::from("RemoteApp was not retained".to_string());
            error.retryable = recovery.retryable();
            return Err(error);
        }
    };
    let security = match outcome.security.take() {
        Some(security) => security,
        None => {
            let session_id = outcome
                .report
                .sessions
                .first()
                .map(|session| session.session_id.clone());
            let recovery = recover_post_success_failure(
                config,
                &operation_directory,
                operation_id,
                &runtime_session_id,
                session_id.as_deref(),
                project_directory.as_deref(),
                Some(client),
            )
            .await;
            let mut error = WindowsOperationFailure::from(
                "RemoteApp operation security was not retained".to_string(),
            );
            error.retryable = recovery.retryable();
            return Err(error);
        }
    };
    let Some(studio_session) = outcome.report.sessions.first().cloned() else {
        let recovery = recover_post_success_failure(
            config,
            &operation_directory,
            operation_id,
            &runtime_session_id,
            None,
            project_directory.as_deref(),
            Some(client),
        )
        .await;
        let mut error = WindowsOperationFailure::from(
            "Windows did not retain the launched Studio session identity".to_string(),
        );
        error.retryable = recovery.retryable();
        return Err(error);
    };
    let registration =
        super::sessions::register_launch_client(super::sessions::LaunchClientRegistration {
            config,
            expected_version: version,
            report: &outcome.report.sessions,
            client,
            report_path,
            control_path,
            security,
            previous_report: outcome.authenticated,
            project_access,
        });
    if let Err(error) = registration {
        let recovery = recover_post_success_failure(
            config,
            &operation_directory,
            operation_id,
            &runtime_session_id,
            Some(&studio_session.session_id),
            project_directory.as_deref(),
            None,
        )
        .await;
        let mut error = error;
        error.retryable = recovery.retryable();
        return Err(error);
    }
    if let Err(error) = link_studio_runtime_session(&runtime_session_id, &studio_session.session_id)
    {
        let recovery = recover_post_success_failure(
            config,
            &operation_directory,
            operation_id,
            &runtime_session_id,
            Some(&studio_session.session_id),
            project_directory.as_deref(),
            None,
        )
        .await;
        let mut error = WindowsOperationFailure::from(error.message);
        error.retryable = recovery.retryable();
        return Err(error);
    }
    Ok(())
}

async fn recover_post_success_failure(
    config: &AppConfig,
    operation_directory: &Path,
    operation_id: &str,
    runtime_session_id: &str,
    studio_session_id: Option<&str>,
    project_directory: Option<&Path>,
    mut remote_app: Option<super::remote_app::RemoteAppProcess>,
) -> PostSuccessCleanup {
    let mut remote_app_cleanup = "not_owned";
    if let Some(client) = remote_app.as_mut() {
        remote_app_cleanup = match client.try_wait() {
            Ok(Some(_)) => "succeeded",
            Ok(None) => {
                if client.kill().and_then(|()| client.wait()).is_ok() {
                    "succeeded"
                } else {
                    "failed"
                }
            }
            Err(_) => "failed",
        };
    }

    let studio_stop = if let Some(studio_session_id) = studio_session_id {
        match super::sessions::stop_registered_client(studio_session_id).await {
            Ok(_) => "succeeded",
            Err(_) => "failed",
        }
    } else {
        "not_available"
    };
    let runtime_stop_result = stop_runtime_session_for_recovery(config, runtime_session_id).await;
    let runtime_stop_code = runtime_stop_result.as_ref().err().map(|error| error.code);
    let runtime_stop = if runtime_stop_result.is_ok() {
        "succeeded"
    } else {
        "failed"
    };

    let session_observation = match studio_session_id {
        Some(studio_session_id) => match super::sessions::list(config).await {
            Ok(sessions)
                if sessions
                    .iter()
                    .any(|session| session.session_id == studio_session_id) =>
            {
                SessionObservation::Present
            }
            Ok(_) => SessionObservation::Absent,
            Err(_) => SessionObservation::Unverified,
        },
        None => SessionObservation::Unverified,
    };
    let (lock_cleanup, locks_removed) = cleanup_dead_launch_lock(
        config,
        studio_session_id,
        project_directory,
        session_observation,
    );

    let mut cleanup = PostSuccessCleanup {
        remote_app_cleanup,
        studio_stop,
        runtime_stop,
        runtime_stop_code,
        session_observation,
        lock_cleanup,
        locks_removed,
        report_written: false,
    };
    cleanup.report_written = write_post_success_cleanup_report(
        operation_directory,
        operation_id,
        studio_session_id.unwrap_or_default(),
        runtime_session_id,
        &cleanup,
    );
    cleanup
}

#[cfg(target_os = "linux")]
async fn stop_runtime_session_for_recovery(
    config: &AppConfig,
    runtime_session_id: &str,
) -> Result<(), crate::contracts::BackendError> {
    super::runtime::stop(config, runtime_session_id).await
}

#[cfg(not(target_os = "linux"))]
async fn stop_runtime_session_for_recovery(
    _config: &AppConfig,
    _runtime_session_id: &str,
) -> Result<(), crate::contracts::BackendError> {
    Err(crate::contracts::BackendError::operation(
        crate::contracts::BackendId::LinuxWinboat,
        crate::contracts::CapabilityId::RuntimeStop,
        "the WinBoat Studio Runtime adapter requires Linux",
    ))
}

fn cleanup_dead_launch_lock(
    config: &AppConfig,
    studio_session_id: Option<&str>,
    project_directory: Option<&Path>,
    observation: SessionObservation,
) -> (&'static str, usize) {
    let Some(studio_session_id) = studio_session_id else {
        return ("not_available", 0);
    };
    let Some(process_id) = studio_process_id(studio_session_id) else {
        return ("retained_invalid_identity", 0);
    };
    match observation {
        SessionObservation::Absent => {
            let directory =
                project_directory.unwrap_or_else(|| Path::new(&config.shared_directory));
            match super::project_locks::remove_dead_process_lock(directory, process_id) {
                Ok(count) => ("succeeded", count),
                Err(_) => ("failed", 0),
            }
        }
        SessionObservation::Present => ("retained_live_session", 0),
        SessionObservation::Unverified => ("retained_unverified_session", 0),
    }
}

fn studio_process_id(session_id: &str) -> Option<u32> {
    session_id
        .strip_prefix("studio-")?
        .split_once('-')?
        .0
        .parse()
        .ok()
}

fn write_post_success_cleanup_report(
    operation_directory: &Path,
    operation_id: &str,
    studio_session_id: &str,
    runtime_session_id: &str,
    cleanup: &PostSuccessCleanup,
) -> bool {
    if validate_operation_id(operation_id).is_err() {
        return false;
    }
    let report = PostSuccessCleanupReport {
        schema_version: crate::contracts::CONTRACT_SCHEMA_VERSION,
        operation_id,
        studio_session_id,
        runtime_session_id,
        remote_app_cleanup: cleanup.remote_app_cleanup,
        studio_stop: cleanup.studio_stop,
        runtime_stop: cleanup.runtime_stop,
        runtime_stop_code: cleanup.runtime_stop_code,
        session_observation: match cleanup.session_observation {
            SessionObservation::Absent => "absent",
            SessionObservation::Present => "present",
            SessionObservation::Unverified => "unverified",
        },
        lock_cleanup: cleanup.lock_cleanup,
        locks_removed: cleanup.locks_removed,
        report_written: true,
        retryable: cleanup.retryable(),
    };
    let Ok(content) = serde_json::to_vec_pretty(&report) else {
        return false;
    };
    let path = operation_directory.join(format!("{operation_id}.cleanup.json"));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let Ok(mut file) = options.open(path) else {
        return false;
    };
    file.write_all(&content)
        .and_then(|()| file.sync_all())
        .is_ok()
}

#[cfg(target_os = "linux")]
async fn prepare_studio_runtime_session(
    config: &AppConfig,
    project_mpr_path: Option<&str>,
) -> Result<String, WindowsOperationFailure> {
    super::runtime::prepare_studio_session(config, project_mpr_path, 3_600)
        .await
        .map_err(|error| WindowsOperationFailure::from(error.message))
}

#[cfg(not(target_os = "linux"))]
async fn prepare_studio_runtime_session(
    _config: &AppConfig,
    _project_mpr_path: Option<&str>,
) -> Result<String, WindowsOperationFailure> {
    Err(WindowsOperationFailure::from(
        "the WinBoat Studio Runtime adapter requires Linux".to_string(),
    ))
}

#[cfg(target_os = "linux")]
async fn stop_runtime_session(config: &AppConfig, runtime_session_id: &str) {
    let _ = super::runtime::stop(config, runtime_session_id).await;
}

#[cfg(not(target_os = "linux"))]
async fn stop_runtime_session(_config: &AppConfig, _runtime_session_id: &str) {}

#[cfg(target_os = "linux")]
fn link_studio_runtime_session(
    runtime_session_id: &str,
    studio_session_id: &str,
) -> Result<(), crate::contracts::BackendError> {
    super::runtime::link_studio_session(runtime_session_id, studio_session_id)
}

#[cfg(not(target_os = "linux"))]
fn link_studio_runtime_session(
    _runtime_session_id: &str,
    _studio_session_id: &str,
) -> Result<(), crate::contracts::BackendError> {
    Err(crate::contracts::BackendError::operation(
        crate::contracts::BackendId::LinuxWinboat,
        crate::contracts::CapabilityId::RuntimeStart,
        "the WinBoat Studio Runtime adapter requires Linux",
    ))
}

async fn abort_incomplete_launch(
    config: &AppConfig,
    selected: &crate::models::StudioVersion,
    session: &WindowsStudioSessionReport,
    operation_id: &str,
    attempt: usize,
) -> Result<(), WindowsOperationFailure> {
    let (process_id, started_ticks) = validate_incomplete_launch_identity(selected, session)?;
    let abort_id = format!("{operation_id}-abort-{}", attempt + 1);
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{abort_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = abort_studio_launch_script(
        &selected.executable_path,
        &windows_report_path,
        &config.mendix_install_root,
        process_id,
        started_ticks,
    );
    let command = write_command_script(config, &abort_id, &script)?;
    run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: "Clean up incomplete Studio Pro launch",
            report_path: &report_path,
            timeout_seconds: FAILED_LAUNCH_ABORT_TIMEOUT_SECONDS,
            operation: "cleaning up incomplete Studio Pro launch",
            keep_remote_app_alive: false,
            cancellation: None,
            project_access: None,
        },
        |_| {},
    )
    .await
    .map(|_| ())
}

fn validate_incomplete_launch_identity(
    selected: &crate::models::StudioVersion,
    session: &WindowsStudioSessionReport,
) -> Result<(u32, i64), WindowsOperationFailure> {
    let remainder = session
        .session_id
        .strip_prefix("studio-")
        .ok_or_else(|| "Windows returned an invalid incomplete launch identity".to_string())?;
    let (process_id, started_ticks) = remainder
        .split_once('-')
        .ok_or_else(|| "Windows returned an invalid incomplete launch identity".to_string())?;
    let process_id = process_id
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "Windows returned an invalid incomplete launch identity".to_string())?;
    let started_ticks = started_ticks
        .parse::<i64>()
        .ok()
        .filter(|value| *value > WINDOWS_EPOCH_TICKS)
        .ok_or_else(|| "Windows returned an invalid incomplete launch identity".to_string())?;
    let started_at = chrono::DateTime::parse_from_rfc3339(&session.started_at)
        .map_err(|_| "Windows returned an invalid incomplete launch start time".to_string())?;
    let reported_ticks = WINDOWS_EPOCH_TICKS
        .checked_add(
            started_at
                .timestamp()
                .checked_mul(TICKS_PER_SECOND)
                .ok_or_else(|| {
                    "Windows returned an invalid incomplete launch start time".to_string()
                })?,
        )
        .and_then(|ticks| ticks.checked_add(i64::from(started_at.timestamp_subsec_nanos() / 100)))
        .ok_or_else(|| "Windows returned an invalid incomplete launch start time".to_string())?;
    if session.version != selected.version
        || session.process_id != process_id
        || reported_ticks != started_ticks
        || session.has_window
    {
        return Err(
            "Windows returned an inconsistent incomplete launch identity"
                .to_string()
                .into(),
        );
    }
    Ok((process_id, started_ticks))
}

fn ensure_control_path_available(control_path: &Path) -> Result<(), String> {
    let mut temporary = control_path.as_os_str().to_os_string();
    temporary.push(".tmp");
    for path in [control_path.to_path_buf(), temporary.into()] {
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(format!(
                    "the Studio Pro control path already exists: {}",
                    path.display()
                ))
            }
            Err(error) => {
                return Err(format!(
                    "the Studio Pro control path could not be inspected: {error}"
                ))
            }
        }
    }
    Ok(())
}

pub async fn install_studio<F>(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
    windows_installer_path: &str,
    expected_sha256: &str,
    cancellation: CancellationToken,
    mut on_progress: F,
) -> Result<String, WindowsOperationFailure>
where
    F: FnMut(StudioInstallProgress) + Send,
{
    validate_version(version)?;
    validate_operation_id(operation_id)?;
    validate_sha256(expected_sha256)?;
    ensure_no_registered_remote_app()?;
    ensure_guest_online(config).await?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;

    let script = install_script(
        windows_installer_path,
        &windows_report_path,
        &config.mendix_install_root,
        version,
        expected_sha256,
        &config.windows_shared_directory,
    );
    // Keep the exact script next to other commands so a failed installation can
    // be diagnosed without exposing the Windows password or FreeRDP arguments.
    let command = write_command_script(config, operation_id, &script)?;
    let label = format!("Install Studio Pro {version}");
    let operation = crate::tr!("operation-studio-install");
    on_progress(StudioInstallProgress {
        phase: StudioInstallPhase::Staging,
        percentage: Some(0.0),
        estimated: false,
    });
    let mut progress_state = InstallProgressState::default();
    let outcome = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: &label,
            report_path: &report_path,
            timeout_seconds: INSTALL_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive: false,
            cancellation: Some(&cancellation),
            project_access: None,
        },
        |report| {
            let phase = match report.state {
                WindowsOperationState::Staging => StudioInstallPhase::Staging,
                WindowsOperationState::Installing => StudioInstallPhase::Installing,
                WindowsOperationState::Finalizing => StudioInstallPhase::Finalizing,
                WindowsOperationState::Verifying => StudioInstallPhase::Verifying,
                _ => return,
            };
            progress_state.observe(phase, report.percentage, report.estimated, &mut on_progress);
        },
    )
    .await?;
    let executable_path = outcome
        .report
        .executable_path
        .filter(|path| !path.is_empty())
        .ok_or_else(|| crate::tr!("error-install-path-missing"))?;
    progress_state.complete(&mut on_progress);
    super::version_cache::invalidate();
    Ok(executable_path)
}

#[derive(Default)]
struct InstallProgressState {
    installing: bool,
    finalizing: bool,
    verifying: bool,
    verification_complete: bool,
}

impl InstallProgressState {
    fn observe<F>(
        &mut self,
        phase: StudioInstallPhase,
        percentage: Option<f64>,
        estimated: bool,
        on_progress: &mut F,
    ) where
        F: FnMut(StudioInstallProgress),
    {
        let Some(percentage) = percentage else {
            return;
        };
        match phase {
            StudioInstallPhase::Staging => {}
            StudioInstallPhase::Installing => self.installing = true,
            StudioInstallPhase::Finalizing => {
                self.ensure_installing(on_progress);
                self.finalizing = true;
            }
            StudioInstallPhase::Verifying => {
                self.ensure_installing(on_progress);
                self.ensure_finalizing(on_progress);
                self.verifying = true;
                self.verification_complete = percentage >= 100.0;
            }
        }
        on_progress(StudioInstallProgress {
            phase,
            percentage: Some(percentage.clamp(0.0, 100.0)),
            estimated,
        });
    }

    fn complete<F>(&mut self, on_progress: &mut F)
    where
        F: FnMut(StudioInstallProgress),
    {
        self.ensure_installing(on_progress);
        self.ensure_finalizing(on_progress);
        if !self.verifying || !self.verification_complete {
            on_progress(StudioInstallProgress {
                phase: StudioInstallPhase::Verifying,
                percentage: Some(100.0),
                estimated: false,
            });
            self.verifying = true;
            self.verification_complete = true;
        }
    }

    fn ensure_installing<F>(&mut self, on_progress: &mut F)
    where
        F: FnMut(StudioInstallProgress),
    {
        if !self.installing {
            on_progress(StudioInstallProgress {
                phase: StudioInstallPhase::Installing,
                percentage: Some(100.0),
                estimated: true,
            });
            self.installing = true;
        }
    }

    fn ensure_finalizing<F>(&mut self, on_progress: &mut F)
    where
        F: FnMut(StudioInstallProgress),
    {
        if !self.finalizing {
            on_progress(StudioInstallProgress {
                phase: StudioInstallPhase::Finalizing,
                percentage: Some(100.0),
                estimated: true,
            });
            self.finalizing = true;
        }
    }
}

pub async fn launch_uninstaller(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
) -> Result<(), WindowsOperationFailure> {
    validate_version(version)?;
    validate_operation_id(operation_id)?;
    ensure_no_registered_remote_app()?;
    ensure_guest_online(config).await?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = uninstall_script(
        &config.mendix_data_root,
        &config.mendix_install_root,
        version,
        &windows_report_path,
    );
    let command = write_command_script(config, operation_id, &script)?;
    let label = format!("Uninstall Studio Pro {version}");
    let operation = crate::tr!("operation-studio-uninstall");
    run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: &label,
            report_path: &report_path,
            timeout_seconds: UNINSTALL_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive: false,
            cancellation: None,
            project_access: None,
        },
        |_| {},
    )
    .await?;
    super::version_cache::invalidate();
    Ok(())
}

fn ensure_no_registered_remote_app() -> Result<(), WindowsOperationFailure> {
    if !super::sessions::registered_client_sessions().is_empty() {
        return Err(WindowsOperationFailure {
            message: "a connected Studio Pro RemoteApp session is still running".to_string(),
            exit_code: None,
            retryable: false,
            failure_kind: None,
        });
    }
    Ok(())
}

pub fn open_linux_folder(path: &str) -> Result<(), String> {
    let directory = Path::new(path);
    if !directory.is_dir() {
        return Err(crate::tr!("error-directory-not-found", path = path));
    }
    Command::new("xdg-open")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| crate::tr!("error-file-manager-open", error = error))
}

pub(super) struct PreparedCommand {
    pub(super) path: PathBuf,
    pub(super) sha256: String,
}

pub(super) fn write_command_script(
    config: &AppConfig,
    name: &str,
    content: &str,
) -> Result<PreparedCommand, String> {
    let command_directory = secure_shared_directory(config, ".mendimaru/commands")?;
    let safe_name = safe_operation_name(name);
    let path = command_directory.join(format!("{safe_name}.ps1"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|error| crate::tr!("error-command-script-save", error = error))?;
    file.write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| crate::tr!("error-command-script-save", error = error))?;
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    Ok(PreparedCommand { path, sha256 })
}

pub(super) fn secure_shared_directory(
    config: &AppConfig,
    relative: &str,
) -> Result<PathBuf, String> {
    let shared = Path::new(&config.shared_directory);
    let shared_metadata = fs::symlink_metadata(shared)
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    if !shared_metadata.is_dir() || shared_metadata.file_type().is_symlink() {
        return Err(crate::tr!(
            "error-secure-shared-directory",
            error = "the configured shared root is not a direct directory"
        ));
    }
    let directory = shared.join(relative);
    fs::create_dir_all(&directory)
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    let canonical_shared = shared
        .canonicalize()
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    let canonical_directory = directory
        .canonicalize()
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    if !canonical_directory.starts_with(&canonical_shared) {
        return Err(crate::tr!(
            "error-secure-shared-directory",
            error = "the application directory escapes the shared root"
        ));
    }
    let mut current = shared.to_path_buf();
    for component in Path::new(relative).components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(crate::tr!(
                "error-secure-shared-directory",
                error = "a shared application directory is a symbolic link"
            ));
        }
    }
    Ok(directory)
}

fn validate_operation_id(value: &str) -> Result<(), String> {
    if !value.is_empty() && value.len() <= 160 && safe_operation_name(value) == value {
        Ok(())
    } else {
        Err(crate::tr!("error-operation-id-invalid"))
    }
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(crate::tr!("error-installer-sha256-invalid"))
    }
}

fn safe_operation_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
        .collect()
}

#[cfg(test)]
mod progress_tests {
    use super::{
        cleanup_dead_launch_lock, validate_incomplete_launch_identity, validate_operation_id,
        write_post_success_cleanup_report, InstallProgressState, PostSuccessCleanup,
        SessionObservation,
    };
    use crate::contracts::BackendErrorCode;
    use crate::models::{StudioInstallPhase, StudioInstallProgress, StudioVersion};
    use crate::winboat::operation::WindowsStudioSessionReport;
    use std::fs;
    use std::path::Path;

    #[test]
    fn successful_install_synthesizes_transient_phases_missed_between_polls() {
        let mut state = InstallProgressState::default();
        let mut updates = Vec::new();
        state.complete(&mut |update| updates.push(update));

        assert_eq!(
            updates
                .iter()
                .map(|update| update.phase)
                .collect::<Vec<_>>(),
            [
                StudioInstallPhase::Installing,
                StudioInstallPhase::Finalizing,
                StudioInstallPhase::Verifying,
            ]
        );
        assert!(updates
            .iter()
            .all(|update| update.percentage == Some(100.0)));
        assert!(!updates.last().expect("verification update").estimated);
    }

    #[test]
    fn accepts_only_bounded_filename_safe_host_operation_ids() {
        assert!(validate_operation_id("install-11.12.2-0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_operation_id("").is_err());
        assert!(validate_operation_id("../operation").is_err());
        assert!(validate_operation_id(&"a".repeat(161)).is_err());
    }

    #[test]
    fn verifying_report_preserves_order_and_is_completed_exactly_once() {
        let mut state = InstallProgressState::default();
        let mut updates: Vec<StudioInstallProgress> = Vec::new();
        state.observe(
            StudioInstallPhase::Verifying,
            Some(0.0),
            false,
            &mut |update| updates.push(update),
        );
        state.complete(&mut |update| updates.push(update));
        state.complete(&mut |update| updates.push(update));

        assert_eq!(
            updates
                .iter()
                .map(|update| (update.phase, update.percentage))
                .collect::<Vec<_>>(),
            [
                (StudioInstallPhase::Installing, Some(100.0)),
                (StudioInstallPhase::Finalizing, Some(100.0)),
                (StudioInstallPhase::Verifying, Some(0.0)),
                (StudioInstallPhase::Verifying, Some(100.0)),
            ]
        );
    }

    #[test]
    fn incomplete_launch_cleanup_requires_one_consistent_exact_process_identity() {
        let selected = StudioVersion {
            version: "11.12.2".into(),
            display_name: "Studio Pro 11.12.2".into(),
            executable_path: r"C:\Program Files\Mendix\11.12.2\modeler\studiopro.exe".into(),
            install_root: r"C:\Program Files\Mendix\11.12.2".into(),
            source: "fixture".into(),
            removable: true,
        };
        let valid = WindowsStudioSessionReport {
            session_id: "studio-4242-639223488000000000".into(),
            version: "11.12.2".into(),
            process_id: 4242,
            started_at: "2026-08-15T00:00:00Z".into(),
            project_name: None,
            has_window: false,
        };

        assert_eq!(
            validate_incomplete_launch_identity(&selected, &valid).expect("valid identity"),
            (4242, 639_223_488_000_000_000)
        );

        let mut mismatch = valid.clone();
        mismatch.process_id = 4243;
        assert!(validate_incomplete_launch_identity(&selected, &mismatch).is_err());

        let mut ready = valid;
        ready.has_window = true;
        assert!(validate_incomplete_launch_identity(&selected, &ready).is_err());
    }

    #[test]
    fn dead_launch_locks_are_removed_only_after_authoritative_absence() {
        let workspace = tempfile::tempdir().expect("workspace");
        let project = tempfile::tempdir().expect("exact project directory");
        let config = cleanup_config(workspace.path());
        let session_id = "studio-4242-639240248846965363";
        write_dead_lock(project.path(), "Project.mpr.lock", 4242);
        write_dead_lock(project.path(), "Other.mpr.lock", 9000);
        write_dead_lock(workspace.path(), "Shared.mpr.lock", 4242);

        let (result, removed) = cleanup_dead_launch_lock(
            &config,
            Some(session_id),
            Some(project.path()),
            SessionObservation::Absent,
        );
        assert_eq!(result, "succeeded");
        assert_eq!(removed, 1);
        assert!(!project.path().join("Project.mpr.lock").exists());
        assert!(project.path().join("Other.mpr.lock").exists());
        assert!(workspace.path().join("Shared.mpr.lock").exists());

        write_dead_lock(project.path(), "Project.mpr.lock", 4242);
        let (live, removed) = cleanup_dead_launch_lock(
            &config,
            Some(session_id),
            Some(project.path()),
            SessionObservation::Present,
        );
        assert_eq!(live, "retained_live_session");
        assert_eq!(removed, 0);
        assert!(project.path().join("Project.mpr.lock").exists());

        let (unverified, removed) = cleanup_dead_launch_lock(
            &config,
            Some(session_id),
            Some(project.path()),
            SessionObservation::Unverified,
        );
        assert_eq!(unverified, "retained_unverified_session");
        assert_eq!(removed, 0);
        assert!(project.path().join("Project.mpr.lock").exists());
    }

    #[test]
    fn post_success_cleanup_reports_runtime_stop_failure_without_paths() {
        let directory = tempfile::tempdir().expect("operation directory");
        let cleanup = PostSuccessCleanup {
            remote_app_cleanup: "succeeded",
            studio_stop: "not_registered",
            runtime_stop: "failed",
            runtime_stop_code: Some(BackendErrorCode::RuntimeComposeRecoveryFailed),
            session_observation: SessionObservation::Absent,
            lock_cleanup: "succeeded",
            locks_removed: 1,
            report_written: false,
        };

        assert!(write_post_success_cleanup_report(
            directory.path(),
            "launch-11.12.2-0123456789abcdef0123456789abcdef",
            "studio-4242-639240248846965363",
            &format!("runtime_{}", "a".repeat(32)),
            &cleanup,
        ));
        let serialized = fs::read_to_string(
            directory
                .path()
                .join("launch-11.12.2-0123456789abcdef0123456789abcdef.cleanup.json"),
        )
        .expect("post-success cleanup report");
        assert!(!serialized.contains(directory.path().to_string_lossy().as_ref()));
        assert!(serialized.contains("\"runtimeStop\": \"failed\""));
        assert!(serialized.contains("\"runtimeStopCode\": \"runtime_compose_recovery_failed\""));
        assert!(!cleanup.retryable());

        let mut recovered = PostSuccessCleanup {
            remote_app_cleanup: "not_owned",
            studio_stop: "succeeded",
            runtime_stop: "succeeded",
            runtime_stop_code: None,
            session_observation: SessionObservation::Absent,
            lock_cleanup: "succeeded",
            locks_removed: 1,
            report_written: true,
        };
        assert!(recovered.retryable());
        recovered.report_written = false;
        assert!(!recovered.retryable());
    }

    fn cleanup_config(shared: &Path) -> crate::models::AppConfig {
        crate::models::AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "missing-winboat".into(),
            compose_file: "missing-compose.yml".into(),
            container_runtime: crate::models::ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:9".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 9,
            shared_directory: shared.to_string_lossy().into_owned(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "missing-freerdp".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        }
    }

    fn write_dead_lock(directory: &Path, name: &str, process_id: u32) {
        fs::write(
            directory.join(name),
            format!(
                r#"{{"SessionId":"f205dd03-790d-43fa-a77d-be3b6b7ff1b7","ProcessId":{process_id}}}"#
            ),
        )
        .expect("dead lock fixture");
    }
}
