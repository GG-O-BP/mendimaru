use super::client::installed_versions;
use super::container::ensure_guest_online;
use super::operation::{
    run_windows_operation, WindowsOperationFailure, WindowsOperationOutcome,
    WindowsOperationRequest, WindowsStudioSessionReport,
};
use super::remote_app::RemoteAppProcess;
use super::scripts::{studio_sessions_script, StudioSessionScriptMode};
use super::studio::{secure_shared_directory, write_command_script};
use crate::contracts::{
    StudioConnectionState, StudioProcessState, StudioReconnectUnavailable, StudioSessionStatus,
    CONTRACT_SCHEMA_VERSION,
};
use crate::models::{AppConfig, StudioVersion};
use crate::projects::linux_path_to_windows_share;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SESSION_OPERATION_TIMEOUT_SECONDS: u64 = 90;
const WINDOWS_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;
const MAX_PROJECT_NAME_BYTES: usize = 160;

static SESSION_CLIENTS: OnceLock<Mutex<HashMap<String, RemoteAppProcess>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionIdentity {
    process_id: u32,
    started_ticks: i64,
}

pub(crate) async fn list(
    config: &AppConfig,
) -> Result<Vec<StudioSessionStatus>, WindowsOperationFailure> {
    ensure_guest_online(config).await?;
    let studios = installed_versions(config).await?;
    let outcome = execute(
        config,
        &studios,
        StudioSessionScriptMode::Query,
        "Inspect Studio Pro sessions",
        false,
    )
    .await?;
    normalize_sessions(&studios, outcome.report.sessions)
}

pub(crate) async fn reconnect(
    config: &AppConfig,
    session_id: &str,
) -> Result<(), WindowsOperationFailure> {
    let identity = parse_session_id(session_id)?;
    if client_is_connected(session_id) {
        return Err(failure(crate::tr!(
            "error-studio-session-already-connected"
        )));
    }
    ensure_guest_online(config).await?;
    let studios = installed_versions(config).await?;
    let mut outcome = execute(
        config,
        &studios,
        StudioSessionScriptMode::Reconnect {
            process_id: identity.process_id,
            started_ticks: identity.started_ticks,
        },
        "Reconnect Studio Pro session",
        true,
    )
    .await?;
    let sessions = normalize_sessions(&studios, outcome.report.sessions.clone())?;
    let exact = sessions
        .into_iter()
        .any(|session| session.session_id == session_id && session.reconnectable);
    if !exact {
        terminate_outcome_client(&mut outcome);
        return Err(failure(
            "Windows did not confirm the selected reconnectable Studio Pro session",
        ));
    }
    let client = outcome
        .remote_app
        .take()
        .ok_or_else(|| failure("the RemoteApp reconnect client was not retained"))?;
    register_client(session_id, client)?;
    Ok(())
}

pub(crate) async fn stop(
    config: &AppConfig,
    session_id: &str,
) -> Result<(), WindowsOperationFailure> {
    let identity = parse_session_id(session_id)?;
    ensure_guest_online(config).await?;
    let studios = installed_versions(config).await?;
    execute(
        config,
        &studios,
        StudioSessionScriptMode::Stop {
            process_id: identity.process_id,
            started_ticks: identity.started_ticks,
        },
        "Close Studio Pro session",
        false,
    )
    .await?;
    disconnect_client(session_id);
    Ok(())
}

pub(super) fn register_launch_client(
    expected_version: &str,
    report: &[WindowsStudioSessionReport],
    client: RemoteAppProcess,
) -> Result<(), WindowsOperationFailure> {
    let Some(session) = report.first() else {
        let mut client = client;
        let _ = client.kill();
        let _ = client.wait();
        return Err(failure(
            "Windows did not return the launched Studio Pro session identity",
        ));
    };
    let identity = match parse_session_id(&session.session_id) {
        Ok(identity) => identity,
        Err(error) => {
            terminate_client(client);
            return Err(error);
        }
    };
    let started_at =
        DateTime::parse_from_rfc3339(&session.started_at).map(|value| value.with_timezone(&Utc));
    if report.len() != 1
        || identity.process_id != session.process_id
        || session.version != expected_version
        || !session.has_window
        || started_at.ok().and_then(datetime_ticks) != Some(identity.started_ticks)
    {
        terminate_client(client);
        return Err(failure(
            "Windows returned an invalid launched Studio Pro session identity",
        ));
    }
    register_client(&session.session_id, client)
}

fn terminate_client(mut client: RemoteAppProcess) {
    let _ = client.kill();
    let _ = client.wait();
}

fn normalize_sessions(
    studios: &[StudioVersion],
    sessions: Vec<WindowsStudioSessionReport>,
) -> Result<Vec<StudioSessionStatus>, WindowsOperationFailure> {
    if sessions.len() > 64 {
        return Err(failure("Windows returned too many Studio Pro sessions"));
    }
    let versions = studios
        .iter()
        .map(|studio| studio.version.as_str())
        .collect::<HashSet<_>>();
    let mut identifiers = HashSet::new();
    let mut normalized = Vec::with_capacity(sessions.len());
    for session in sessions {
        let identity = parse_session_id(&session.session_id)?;
        if identity.process_id != session.process_id
            || !versions.contains(session.version.as_str())
            || !identifiers.insert(session.session_id.clone())
        {
            return Err(failure("Windows returned an invalid Studio Pro session"));
        }
        crate::platform::validate_version(&session.version).map_err(failure)?;
        let started_at = DateTime::parse_from_rfc3339(&session.started_at)
            .map_err(|_| failure("Windows returned an invalid Studio Pro start time"))?
            .with_timezone(&Utc);
        if datetime_ticks(started_at) != Some(identity.started_ticks) {
            return Err(failure(
                "the Studio Pro process identity does not match its start time",
            ));
        }
        let project_name = session
            .project_name
            .filter(|name| safe_project_name(name))
            .map(|name| name.trim().to_string());
        let connected = client_is_connected(&session.session_id);
        let (connection, reconnectable, reconnect_unavailable) = if connected {
            (
                StudioConnectionState::Connected,
                false,
                Some(StudioReconnectUnavailable::AlreadyConnected),
            )
        } else if session.has_window {
            (StudioConnectionState::Disconnected, true, None)
        } else {
            (
                StudioConnectionState::Disconnected,
                false,
                Some(StudioReconnectUnavailable::WindowUnavailable),
            )
        };
        normalized.push(StudioSessionStatus {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            session_id: session.session_id,
            version: session.version,
            state: StudioProcessState::Running,
            process_id: Some(session.process_id),
            started_at: Some(started_at),
            project_name,
            connection,
            reconnectable,
            reconnect_unavailable,
        });
    }
    normalized.sort_by_key(|session| std::cmp::Reverse(session.started_at));
    Ok(normalized)
}

async fn execute(
    config: &AppConfig,
    studios: &[StudioVersion],
    mode: StudioSessionScriptMode,
    label: &str,
    keep_remote_app_alive: bool,
) -> Result<WindowsOperationOutcome, WindowsOperationFailure> {
    let identifier = session_operation_id()?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{identifier}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = studio_sessions_script(
        studios,
        mode,
        &windows_report_path,
        &config.mendix_install_root,
    )?;
    let command = write_command_script(config, &identifier, &script)?;
    let cleanup = SessionArtifacts::new(command.path.clone(), report_path.clone());
    let operation = label.to_string();
    let result = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label,
            report_path: &report_path,
            timeout_seconds: SESSION_OPERATION_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive,
        },
        |_| {},
    )
    .await;
    drop(cleanup);
    result
}

fn session_operation_id() -> Result<String, WindowsOperationFailure> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| failure(format!("could not create a session operation ID: {error}")))?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("session-{suffix}"))
}

fn parse_session_id(value: &str) -> Result<SessionIdentity, WindowsOperationFailure> {
    let remainder = value
        .strip_prefix("studio-")
        .ok_or_else(|| failure("the Studio Pro session identifier is invalid"))?;
    let (process_id, started_ticks) = remainder
        .split_once('-')
        .ok_or_else(|| failure("the Studio Pro session identifier is invalid"))?;
    let process_id = process_id
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| failure("the Studio Pro session identifier is invalid"))?;
    let started_ticks = started_ticks
        .parse::<i64>()
        .ok()
        .filter(|value| *value > WINDOWS_EPOCH_TICKS)
        .ok_or_else(|| failure("the Studio Pro session identifier is invalid"))?;
    Ok(SessionIdentity {
        process_id,
        started_ticks,
    })
}

fn datetime_ticks(value: DateTime<Utc>) -> Option<i64> {
    WINDOWS_EPOCH_TICKS
        .checked_add(value.timestamp().checked_mul(TICKS_PER_SECOND)?)?
        .checked_add(i64::from(value.timestamp_subsec_nanos() / 100))
}

fn safe_project_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= MAX_PROJECT_NAME_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}

fn clients() -> Result<
    std::sync::MutexGuard<'static, HashMap<String, RemoteAppProcess>>,
    WindowsOperationFailure,
> {
    SESSION_CLIENTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| failure("the Studio Pro connection registry is unavailable"))
}

fn client_is_connected(session_id: &str) -> bool {
    let Ok(mut clients) = clients() else {
        return false;
    };
    clients.retain(|_, client| match client.try_wait() {
        Ok(Some(_)) => false,
        Ok(None) | Err(_) => true,
    });
    clients.contains_key(session_id)
}

fn register_client(
    session_id: &str,
    client: RemoteAppProcess,
) -> Result<(), WindowsOperationFailure> {
    let mut clients = match clients() {
        Ok(clients) => clients,
        Err(error) => {
            terminate_client(client);
            return Err(error);
        }
    };
    if let Some(mut previous) = clients.insert(session_id.to_string(), client) {
        let _ = previous.kill();
        let _ = previous.wait();
    }
    Ok(())
}

fn disconnect_client(session_id: &str) {
    if let Ok(mut clients) = clients() {
        if let Some(mut client) = clients.remove(session_id) {
            let _ = client.kill();
            let _ = client.wait();
        }
    }
}

fn terminate_outcome_client(outcome: &mut WindowsOperationOutcome) {
    if let Some(mut client) = outcome.remote_app.take() {
        let _ = client.kill();
        let _ = client.wait();
    }
}

fn failure(message: impl Into<String>) -> WindowsOperationFailure {
    WindowsOperationFailure {
        message: message.into(),
        exit_code: None,
        retryable: false,
    }
}

struct SessionArtifacts {
    paths: Vec<PathBuf>,
}

impl SessionArtifacts {
    fn new(command: PathBuf, report: PathBuf) -> Self {
        let mut report_temporary = report.as_os_str().to_os_string();
        report_temporary.push(".tmp");
        Self {
            paths: vec![command, report, report_temporary.into()],
        }
    }
}

impl Drop for SessionArtifacts {
    fn drop(&mut self) {
        for path in &self.paths {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    let _ = fs::remove_file(path);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        datetime_ticks, list, normalize_sessions, parse_session_id, reconnect, safe_project_name,
        stop,
    };
    use crate::models::StudioVersion;
    use crate::winboat::operation::WindowsStudioSessionReport;
    use chrono::{TimeZone, Utc};

    fn studio(version: &str) -> StudioVersion {
        StudioVersion {
            version: version.into(),
            display_name: format!("Studio Pro {version}"),
            executable_path: format!(r"C:\Program Files\Mendix\{version}\modeler\studiopro.exe"),
            install_root: format!(r"C:\Program Files\Mendix\{version}"),
            source: "fixture".into(),
            removable: true,
        }
    }

    #[test]
    fn validates_pid_and_start_time_as_one_session_identity() {
        let started = Utc
            .with_ymd_and_hms(2026, 8, 15, 3, 0, 0)
            .single()
            .expect("start time");
        let ticks = datetime_ticks(started).expect("Windows ticks");
        let id = format!("studio-4242-{ticks}");
        assert_eq!(parse_session_id(&id).expect("session ID").process_id, 4242);
        let earlier = Utc
            .with_ymd_and_hms(2026, 8, 15, 2, 0, 0)
            .single()
            .expect("earlier start time");
        let earlier_ticks = datetime_ticks(earlier).expect("earlier Windows ticks");

        let sessions = normalize_sessions(
            &[studio("11.13.0"), studio("10.24.9")],
            vec![
                WindowsStudioSessionReport {
                    session_id: format!("studio-3131-{earlier_ticks}"),
                    version: "10.24.9".into(),
                    process_id: 3131,
                    started_at: earlier.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                    project_name: None,
                    has_window: false,
                },
                WindowsStudioSessionReport {
                    session_id: id,
                    version: "11.13.0".into(),
                    process_id: 4242,
                    started_at: started.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                    project_name: Some("Orders".into()),
                    has_window: true,
                },
            ],
        )
        .expect("valid sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].version, "11.13.0");
        assert_eq!(sessions[0].project_name.as_deref(), Some("Orders"));
        assert!(sessions[0].reconnectable);
        assert_eq!(sessions[1].version, "10.24.9");
        assert!(!sessions[1].reconnectable);
        assert_eq!(
            sessions[1].reconnect_unavailable,
            Some(crate::contracts::StudioReconnectUnavailable::WindowUnavailable)
        );
    }

    #[test]
    fn rejects_pid_reuse_unknown_versions_and_path_like_project_names() {
        let started = Utc
            .with_ymd_and_hms(2026, 8, 15, 3, 0, 0)
            .single()
            .expect("start time");
        let ticks = datetime_ticks(started).expect("Windows ticks");
        let fixture = |session_id: String, version: &str| WindowsStudioSessionReport {
            session_id,
            version: version.into(),
            process_id: 4242,
            started_at: started.to_rfc3339(),
            project_name: None,
            has_window: true,
        };

        assert!(normalize_sessions(
            &[studio("11.13.0")],
            vec![fixture(format!("studio-4242-{}", ticks + 1), "11.13.0")]
        )
        .is_err());
        assert!(normalize_sessions(
            &[studio("11.13.0")],
            vec![fixture(format!("studio-4242-{ticks}"), "11.12.2")]
        )
        .is_err());
        assert!(!safe_project_name(r"C:\Users\dev\secret"));
        assert!(safe_project_name("Orders"));
    }

    #[test]
    #[ignore = "queries live WinBoat sessions and rejects a stale identity without mutation"]
    fn live_e2e_lists_sessions_and_rejects_an_ended_identity() {
        crate::i18n::initialize("en-US").expect("English localization");
        let config = crate::config::detect_config().expect("live WinBoat configuration");
        let installed = tauri::async_runtime::block_on(super::super::installed_versions(&config))
            .expect("installed versions");
        let sessions = tauri::async_runtime::block_on(list(&config)).expect("live session list");
        assert!(sessions.iter().all(|session| {
            session.schema_version == crate::contracts::CONTRACT_SCHEMA_VERSION
                && installed
                    .iter()
                    .any(|studio| studio.version == session.version)
        }));

        let stale = "studio-2147483647-638908236000000000";
        let reconnect_error = tauri::async_runtime::block_on(reconnect(&config, stale))
            .expect_err("stale reconnect must fail");
        assert!(reconnect_error.message.contains("already ended"));
        let stop_error = tauri::async_runtime::block_on(stop(&config, stale))
            .expect_err("stale close must fail");
        assert!(stop_error.message.contains("already ended"));
    }
}
