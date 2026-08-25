use super::client::installed_versions_cached;
use super::container::ensure_guest_online;
use super::operation::{
    run_windows_operation, wait_for_followup_windows_operation, WindowsOperationFailure,
    WindowsOperationOutcome, WindowsOperationRequest, WindowsStudioSessionReport,
};
use super::remote_app::RemoteAppProcess;
use super::scripts::{studio_sessions_script, StudioSessionScriptMode};
use super::security::{authenticated_envelope, AuthenticatedPayload, OperationSecurity};
use super::studio::{secure_shared_directory, write_command_script};
use crate::contracts::{
    StudioConnectionState, StudioProcessState, StudioReconnectUnavailable, StudioSessionStatus,
    CONTRACT_SCHEMA_VERSION,
};
use crate::models::{AppConfig, StudioVersion};
use crate::projects::linux_path_to_windows_share;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const SESSION_OPERATION_TIMEOUT_SECONDS: u64 = 90;
const SESSION_CONTROL_TIMEOUT_SECONDS: u64 = 45;
const WINDOWS_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;
const MAX_PROJECT_NAME_BYTES: usize = 160;

static SESSION_CLIENTS: OnceLock<Mutex<HashMap<String, RegisteredClient>>> = OnceLock::new();
static STOPPING_SESSIONS: OnceLock<Mutex<HashMap<String, StudioSessionStatus>>> = OnceLock::new();

struct RegisteredClient {
    process: RemoteAppProcess,
    status: StudioSessionStatus,
    control: RegisteredControl,
}

struct RegisteredControl {
    report_path: PathBuf,
    control_path: PathBuf,
    security: OperationSecurity,
    previous_report: AuthenticatedPayload,
    next_sequence: u64,
    cleanup_report: bool,
}

struct RetainedSessionOperation {
    report_path: PathBuf,
    control_path: PathBuf,
}

struct SessionExecution {
    outcome: WindowsOperationOutcome,
    retained: Option<RetainedSessionOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionIdentity {
    process_id: u32,
    started_ticks: i64,
}

pub(crate) async fn list(
    config: &AppConfig,
) -> Result<Vec<StudioSessionStatus>, WindowsOperationFailure> {
    let registered = registered_client_sessions();
    if !registered.is_empty() {
        return Ok(registered);
    }
    ensure_guest_online(config).await?;
    let studios = installed_versions_cached(config).await?;
    let outcome = execute(
        config,
        &studios,
        StudioSessionScriptMode::Query,
        "Inspect Studio Pro sessions",
        false,
    )
    .await?;
    normalize_sessions(&studios, outcome.outcome.report.sessions)
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
    if !registered_client_sessions().is_empty() {
        return Err(failure(
            "another connected Studio Pro session must be closed before reconnecting",
        ));
    }
    ensure_guest_online(config).await?;
    let studios = installed_versions_cached(config).await?;
    let mut execution = execute(
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
    let sessions = normalize_sessions(&studios, execution.outcome.report.sessions.clone())?;
    let exact = sessions
        .into_iter()
        .find(|session| session.session_id == session_id && session.reconnectable);
    let Some(mut status) = exact else {
        terminate_outcome_client(&mut execution.outcome);
        return Err(failure(
            "Windows did not confirm the selected reconnectable Studio Pro session",
        ));
    };
    let client = execution
        .outcome
        .remote_app
        .take()
        .ok_or_else(|| failure("the RemoteApp reconnect client was not retained"))?;
    let Some(security) = execution.outcome.security.take() else {
        terminate_client(client);
        return Err(failure("the RemoteApp reconnect security was not retained"));
    };
    let Some(retained) = execution.retained.take() else {
        terminate_client(client);
        return Err(failure(
            "the RemoteApp reconnect control paths were not retained",
        ));
    };
    let (report_path, control_path) = retained.release();
    status.connection = StudioConnectionState::Connected;
    status.reconnectable = false;
    status.reconnect_unavailable = Some(StudioReconnectUnavailable::AlreadyConnected);
    register_client(
        session_id,
        client,
        status,
        RegisteredControl {
            report_path,
            control_path,
            security,
            previous_report: execution.outcome.authenticated,
            next_sequence: 1,
            cleanup_report: true,
        },
    )?;
    Ok(())
}

pub(crate) async fn stop(
    config: &AppConfig,
    session_id: &str,
) -> Result<(), WindowsOperationFailure> {
    let identity = parse_session_id(session_id)?;
    if stop_registered_client(session_id).await? {
        return Ok(());
    }
    if !registered_client_sessions().is_empty() {
        return Err(failure(
            "another connected Studio Pro session prevents a second RDP stop operation",
        ));
    }
    ensure_guest_online(config).await?;
    let studios = installed_versions_cached(config).await?;
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
    report_path: PathBuf,
    control_path: PathBuf,
    security: OperationSecurity,
    previous_report: AuthenticatedPayload,
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
    let started_at = DateTime::parse_from_rfc3339(&session.started_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| failure("Windows returned an invalid Studio Pro start time"))?;
    if report.len() != 1
        || identity.process_id != session.process_id
        || session.version != expected_version
        || !session.has_window
        || datetime_ticks(started_at) != Some(identity.started_ticks)
    {
        terminate_client(client);
        return Err(failure(
            "Windows returned an invalid launched Studio Pro session identity",
        ));
    }
    let status = StudioSessionStatus {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        session_id: session.session_id.clone(),
        version: session.version.clone(),
        state: StudioProcessState::Running,
        process_id: Some(session.process_id),
        started_at: Some(started_at),
        project_name: session
            .project_name
            .as_deref()
            .filter(|name| safe_project_name(name))
            .map(|name| name.trim().to_string()),
        connection: StudioConnectionState::Connected,
        reconnectable: false,
        reconnect_unavailable: Some(StudioReconnectUnavailable::AlreadyConnected),
    };
    register_client(
        &session.session_id,
        client,
        status,
        RegisteredControl {
            report_path,
            control_path,
            security,
            previous_report,
            next_sequence: 1,
            cleanup_report: false,
        },
    )
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
) -> Result<SessionExecution, WindowsOperationFailure> {
    let identifier = session_operation_id()?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{identifier}.json"));
    let control_path = operation_directory.join(format!("{identifier}.control.json"));
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
    let script = studio_sessions_script(
        studios,
        mode,
        &windows_report_path,
        &windows_control_path,
        &config.mendix_install_root,
    )?;
    let command = write_command_script(config, &identifier, &script)?;
    let cleanup = SessionArtifacts::new(
        command.path.clone(),
        report_path.clone(),
        control_path.clone(),
    );
    let operation = label.to_string();
    let outcome = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label,
            report_path: &report_path,
            timeout_seconds: SESSION_OPERATION_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive,
            cancellation: None,
        },
        |_| {},
    )
    .await?;
    let retained = keep_remote_app_alive.then(|| cleanup.retain(report_path, control_path));
    Ok(SessionExecution { outcome, retained })
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
    std::sync::MutexGuard<'static, HashMap<String, RegisteredClient>>,
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
    retain_live_clients(&mut clients);
    if clients.contains_key(session_id) {
        return true;
    }
    drop(clients);
    stopping_sessions().is_ok_and(|sessions| sessions.contains_key(session_id))
}

pub(crate) fn registered_client_sessions() -> Vec<StudioSessionStatus> {
    let Ok(mut clients) = clients() else {
        return Vec::new();
    };
    retain_live_clients(&mut clients);
    let mut sessions = clients
        .values()
        .map(|client| client.status.clone())
        .collect::<Vec<_>>();
    drop(clients);
    if let Ok(stopping) = stopping_sessions() {
        for status in stopping.values() {
            if !sessions
                .iter()
                .any(|session| session.session_id == status.session_id)
            {
                sessions.push(status.clone());
            }
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.started_at));
    sessions
}

fn retain_live_clients(clients: &mut HashMap<String, RegisteredClient>) {
    clients.retain(|_, client| {
        match client.process.try_wait() {
            Ok(Some(_)) => return false,
            Err(_) => return true,
            Ok(None) => {}
        }
        match read_session_active(&mut client.control) {
            Ok(false) => {
                terminate_client_process(&mut client.process);
                false
            }
            Ok(true) | Err(_) => true,
        }
    });
}

fn read_session_active(control: &mut RegisteredControl) -> Result<bool, String> {
    let content = fs::read(&control.report_path)
        .map_err(|error| format!("could not read the Studio Pro session report: {error}"))?;
    let authenticated = super::security::authenticate_report(&content, &control.security)
        .map_err(|error| error.to_string())?;
    if authenticated.sequence < control.previous_report.sequence {
        return Err("the Studio Pro session report sequence regressed".to_string());
    }
    if authenticated.sequence == control.previous_report.sequence {
        return Ok(true);
    }
    let report = super::operation::parse_install_report(&authenticated.payload)
        .map_err(|error| error.to_string())?;
    if report.state != super::operation::WindowsOperationState::Succeeded {
        return Err("the Studio Pro session report did not succeed".to_string());
    }
    control.previous_report = authenticated;
    Ok(!report.sessions.is_empty())
}

pub(crate) fn disconnect_all_clients() {
    if let Ok(mut clients) = clients() {
        for (_, mut client) in clients.drain() {
            let _ = client.process.kill();
            let _ = client.process.wait();
        }
    }
}

pub(crate) async fn close_all_registered_clients() {
    let session_ids = registered_client_sessions()
        .into_iter()
        .map(|session| session.session_id)
        .collect::<Vec<_>>();
    for session_id in session_ids {
        let _ = stop_registered_client(&session_id).await;
    }
    disconnect_all_clients();
}

fn stopping_sessions() -> Result<
    std::sync::MutexGuard<'static, HashMap<String, StudioSessionStatus>>,
    WindowsOperationFailure,
> {
    STOPPING_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| failure("the Studio Pro stopping registry is unavailable"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioStopControl<'a> {
    action: &'static str,
    session_id: &'a str,
    process_id: u32,
    started_ticks: i64,
}

pub(crate) async fn stop_registered_client(
    session_id: &str,
) -> Result<bool, WindowsOperationFailure> {
    let mut client = {
        let mut clients = clients()?;
        clients.retain(|_, client| match client.process.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) | Err(_) => true,
        });
        let Some(client) = clients.remove(session_id) else {
            drop(clients);
            if stopping_sessions()?.contains_key(session_id) {
                return Err(failure("the Studio Pro session is already stopping"));
            }
            return Ok(false);
        };
        match stopping_sessions() {
            Ok(mut stopping) => {
                stopping.insert(session_id.to_string(), client.status.clone());
                client
            }
            Err(error) => {
                clients.insert(session_id.to_string(), client);
                return Err(error);
            }
        }
    };

    let result = stop_registered_client_inner(session_id, &mut client).await;
    if result.is_ok() {
        if let Ok(mut stopping) = stopping_sessions() {
            stopping.remove(session_id);
        }
        return Ok(true);
    }

    let connected = match client.process.try_wait() {
        Ok(Some(_)) => false,
        Ok(None) | Err(_) => true,
    };
    if connected {
        match clients() {
            Ok(mut clients) => {
                if let Some(previous) = clients.insert(session_id.to_string(), client) {
                    terminate_client(previous.process);
                }
            }
            Err(_) => terminate_client(client.process),
        }
    } else {
        let _ = client.process.wait();
    }
    if let Ok(mut stopping) = stopping_sessions() {
        stopping.remove(session_id);
    }
    result.map(|()| true)
}

async fn stop_registered_client_inner(
    session_id: &str,
    client: &mut RegisteredClient,
) -> Result<(), WindowsOperationFailure> {
    let identity = parse_session_id(session_id)?;
    if client.status.session_id != session_id
        || client.status.process_id != Some(identity.process_id)
        || client.status.started_at.and_then(datetime_ticks) != Some(identity.started_ticks)
    {
        return Err(failure(
            "the registered Studio Pro process identity is inconsistent",
        ));
    }
    write_stop_control(session_id, identity, &mut client.control)?;
    let (report, authenticated) = wait_for_followup_windows_operation(
        &client.control.report_path,
        &client.control.security,
        &client.control.previous_report,
        &mut client.process,
        SESSION_CONTROL_TIMEOUT_SECONDS,
        "closing Studio Pro",
    )
    .await?;
    if !report.sessions.is_empty() {
        return Err(failure(
            "Windows returned sessions after confirming Studio Pro closed",
        ));
    }
    client.control.previous_report = authenticated;
    for _ in 0..20 {
        match client.process.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => tokio::time::sleep(Duration::from_millis(250)).await,
            Err(_) => break,
        }
    }
    terminate_client_process(&mut client.process);
    Ok(())
}

fn write_stop_control(
    session_id: &str,
    identity: SessionIdentity,
    control: &mut RegisteredControl,
) -> Result<(), WindowsOperationFailure> {
    let payload = serde_json::to_vec(&StudioStopControl {
        action: "studio.stop",
        session_id,
        process_id: identity.process_id,
        started_ticks: identity.started_ticks,
    })
    .map_err(|error| {
        failure(format!(
            "could not serialize the Studio Pro stop request: {error}"
        ))
    })?;
    let envelope = authenticated_envelope(&control.security, control.next_sequence, &payload)
        .map_err(failure)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&control.control_path).map_err(|error| {
        failure(format!(
            "could not create the authenticated Studio Pro control request: {error}"
        ))
    })?;
    if let Err(error) = file.write_all(&envelope).and_then(|()| file.sync_all()) {
        drop(file);
        remove_regular_file(&control.control_path);
        return Err(failure(format!(
            "could not persist the authenticated Studio Pro control request: {error}"
        )));
    }
    control.next_sequence = control
        .next_sequence
        .checked_add(1)
        .ok_or_else(|| failure("the Studio Pro control sequence was exhausted"))?;
    Ok(())
}

fn terminate_client_process(process: &mut RemoteAppProcess) {
    match process.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

fn register_client(
    session_id: &str,
    client: RemoteAppProcess,
    status: StudioSessionStatus,
    control: RegisteredControl,
) -> Result<(), WindowsOperationFailure> {
    let mut clients = match clients() {
        Ok(clients) => clients,
        Err(error) => {
            terminate_client(client);
            return Err(error);
        }
    };
    if let Some(mut previous) = clients.insert(
        session_id.to_string(),
        RegisteredClient {
            process: client,
            status,
            control,
        },
    ) {
        let _ = previous.process.kill();
        let _ = previous.process.wait();
    }
    Ok(())
}

fn disconnect_client(session_id: &str) {
    if let Ok(mut clients) = clients() {
        if let Some(mut client) = clients.remove(session_id) {
            let _ = client.process.kill();
            let _ = client.process.wait();
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
        failure_kind: None,
    }
}

struct SessionArtifacts {
    paths: Vec<PathBuf>,
}

impl SessionArtifacts {
    fn new(command: PathBuf, report: PathBuf, control: PathBuf) -> Self {
        let mut report_temporary = report.as_os_str().to_os_string();
        report_temporary.push(".tmp");
        let mut control_temporary = control.as_os_str().to_os_string();
        control_temporary.push(".tmp");
        Self {
            paths: vec![
                command,
                report,
                report_temporary.into(),
                control,
                control_temporary.into(),
            ],
        }
    }

    fn retain(mut self, report_path: PathBuf, control_path: PathBuf) -> RetainedSessionOperation {
        for path in &self.paths {
            if path != &report_path && path != &control_path {
                remove_regular_file(path);
            }
        }
        self.paths.clear();
        RetainedSessionOperation {
            report_path,
            control_path,
        }
    }
}

impl Drop for SessionArtifacts {
    fn drop(&mut self) {
        for path in &self.paths {
            remove_regular_file(path);
        }
    }
}

impl RetainedSessionOperation {
    fn release(mut self) -> (PathBuf, PathBuf) {
        let paths = (self.report_path.clone(), self.control_path.clone());
        self.report_path.clear();
        self.control_path.clear();
        paths
    }
}

impl Drop for RetainedSessionOperation {
    fn drop(&mut self) {
        remove_operation_artifacts(&self.report_path);
        remove_operation_artifacts(&self.control_path);
    }
}

impl Drop for RegisteredControl {
    fn drop(&mut self) {
        remove_operation_artifacts(&self.control_path);
        if self.cleanup_report {
            remove_operation_artifacts(&self.report_path);
        }
    }
}

fn remove_operation_artifacts(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    remove_regular_file(path);
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    remove_regular_file(Path::new(&temporary));
}

fn remove_regular_file(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            let _ = fs::remove_file(path);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        datetime_ticks, list, normalize_sessions, parse_session_id, read_session_active, reconnect,
        safe_project_name, stop, write_stop_control, RegisteredControl, SessionIdentity,
    };
    use crate::models::StudioVersion;
    use crate::winboat::operation::WindowsStudioSessionReport;
    use crate::winboat::security::{
        authenticate_report, authenticated_report_fixture, OperationSecurity,
    };
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
    fn writes_an_authenticated_exact_identity_stop_request_without_overwriting() {
        let directory = tempfile::tempdir().expect("temporary control directory");
        let control_path = directory.path().join("session.control.json");
        let security = OperationSecurity::fixture();
        let previous_report = authenticate_report(
            authenticated_report_fixture(&security, 2, br#"{"state":"succeeded"}"#).as_bytes(),
            &security,
        )
        .expect("previous report authenticates");
        let mut control = RegisteredControl {
            report_path: directory.path().join("session.json"),
            control_path: control_path.clone(),
            security,
            previous_report,
            next_sequence: 1,
            cleanup_report: false,
        };
        let identity = SessionIdentity {
            process_id: 4242,
            started_ticks: 638_908_128_000_000_000,
        };

        write_stop_control("studio-4242-638908128000000000", identity, &mut control)
            .expect("stop request is written");
        let content = std::fs::read(&control_path).expect("read stop request");
        let authenticated =
            authenticate_report(&content, &control.security).expect("stop request authenticates");
        assert_eq!(authenticated.sequence, 1);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&authenticated.payload)
                .expect("stop payload is JSON"),
            serde_json::json!({
                "action": "studio.stop",
                "sessionId": "studio-4242-638908128000000000",
                "processId": 4242,
                "startedTicks": 638_908_128_000_000_000_i64,
            })
        );
        assert_eq!(control.next_sequence, 2);
        assert!(
            write_stop_control("studio-4242-638908128000000000", identity, &mut control,).is_err(),
            "an existing control request must never be overwritten"
        );
    }

    #[test]
    fn removes_a_disconnected_launch_after_an_authenticated_empty_session_report() {
        let directory = tempfile::tempdir().expect("temporary report directory");
        let report_path = directory.path().join("launch.json");
        let security = OperationSecurity::fixture();
        let running_payload = br#"{"state":"succeeded","message":"Studio Pro window is ready.","percentage":null,"estimated":false,"timestamp":"2026-08-23T03:00:00Z","exitCode":null,"executablePath":null,"error":null,"sessions":[{"sessionId":"studio-4242-638915148000000000","version":"11.12.3","processId":4242,"startedAt":"2026-08-23T03:00:00Z","projectName":null,"hasWindow":true}]}"#;
        let previous_report = authenticate_report(
            authenticated_report_fixture(&security, 2, running_payload).as_bytes(),
            &security,
        )
        .expect("running report authenticates");
        let closed_payload = br#"{"state":"succeeded","message":"Studio Pro session closed.","percentage":null,"estimated":false,"timestamp":"2026-08-23T03:01:00Z","exitCode":null,"executablePath":null,"error":null,"sessions":[]}"#;
        std::fs::write(
            &report_path,
            authenticated_report_fixture(&security, 3, closed_payload),
        )
        .expect("write closed report");
        let mut control = RegisteredControl {
            report_path,
            control_path: directory.path().join("launch.control.json"),
            security,
            previous_report,
            next_sequence: 1,
            cleanup_report: false,
        };

        assert!(!read_session_active(&mut control).expect("closed report is valid"));
        assert_eq!(control.previous_report.sequence, 3);
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
