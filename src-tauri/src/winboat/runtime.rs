use super::container::{
    guest_is_online, recreate_container, runtime_host_binding, storage_mount_identity,
};
use super::operation::{run_windows_operation, WindowsOperationRequest};
use super::scripts::runtime_port_probe_script;
use super::studio::{secure_shared_directory, write_command_script};
use crate::app_paths::AppPaths;
use crate::config::{
    ensure_runtime_port_mapping, restore_file, runtime_port_mapping, snapshot_file, FileSnapshot,
};
use crate::contracts::{
    secure_identifier, ArtifactDescriptor, ArtifactKind, BackendError, BackendErrorCode, BackendId,
    BackendResult, CapabilityId, RuntimeLogBatch, RuntimeMode, RuntimeStartRequest, RuntimeState,
    RuntimeStatus, StudioProcessState, CONTRACT_SCHEMA_VERSION,
};
use crate::models::AppConfig;
use crate::projects::linux_path_to_windows_share;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

const STORE_DIRECTORY: &str = "winboat-runtime";
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const MAX_COMPOSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_GUEST_PORT: u16 = 8080;
const HTTP_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const PORT_DIAGNOSTIC_TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionRecord {
    schema_version: String,
    session_id: String,
    backend: BackendId,
    mode: RuntimeMode,
    studio_session_id: Option<String>,
    studio_state: StudioProcessState,
    studio_process_id: Option<u32>,
    state: RuntimeState,
    http_ready: bool,
    host_port: u16,
    guest_port: u16,
    started_at: DateTime<Utc>,
    readiness_timeout_seconds: u64,
    failure_code: Option<BackendErrorCode>,
    log_artifact: ArtifactDescriptor,
    compose_changed: bool,
    original_compose_sha256: String,
    managed_compose_sha256: String,
    storage_mount_identity: Vec<String>,
}

struct ComposeTransaction {
    snapshot: FileSnapshot,
    committed: bool,
}

impl ComposeTransaction {
    fn new(snapshot: FileSnapshot) -> Self {
        Self {
            snapshot,
            committed: false,
        }
    }

    fn snapshot(&self) -> &FileSnapshot {
        &self.snapshot
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ComposeTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = restore_file(&self.snapshot);
        }
    }
}

pub(crate) fn session_exists(session_id: &str) -> bool {
    validate_runtime_session_id(session_id)
        .ok()
        .and_then(|()| layout().ok())
        .map(|layout| session_record_path(&layout, session_id).is_file())
        .unwrap_or(false)
}

pub(crate) async fn start(
    config: &AppConfig,
    request: &RuntimeStartRequest,
) -> BackendResult<RuntimeStatus> {
    validate_start_request(request)?;
    if !guest_is_online(config).await {
        return Err(runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeGuestOffline,
            true,
            None,
        ));
    }

    let guest_port = request.guest_port.unwrap_or(DEFAULT_GUEST_PORT);
    let compose_path = direct_compose_path(config, CapabilityId::RuntimeStart)?;
    let existing_mapping = runtime_port_mapping(&compose_path, guest_port).map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        )
    })?;
    let mapping_is_prepared = existing_mapping.is_some_and(|mapping| {
        mapping.host_ip == "127.0.0.1" && mapping.host_port.is_none() && mapping.protocol == "tcp"
    });
    let live_mapping_is_prepared = if mapping_is_prepared {
        runtime_host_binding(config, guest_port)
            .await
            .is_ok_and(|binding| binding.host_ip == "127.0.0.1" && binding.guest_port == guest_port)
    } else {
        false
    };
    if request.studio_session_id.is_some() && !live_mapping_is_prepared {
        return Err(runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            true,
            None,
        ));
    }

    let session_id = secure_identifier("runtime")?;
    let layout = layout().map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::OperationFailed,
            false,
            None,
        )
    })?;
    let directory = session_directory(&layout, &session_id).map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::OperationFailed,
            false,
            None,
        )
    })?;
    let original_compose = read_direct_bounded(&compose_path, MAX_COMPOSE_BYTES).map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        )
    })?;
    let original_compose_sha256 = format!("{:x}", Sha256::digest(&original_compose));
    write_private_file(&directory.join("compose.original.yml"), &original_compose).map_err(
        |_| {
            runtime_error(
                CapabilityId::RuntimeStart,
                BackendErrorCode::OperationFailed,
                false,
                None,
            )
        },
    )?;
    let snapshot = snapshot_file(&compose_path).map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        )
    })?;
    let mut transaction = ComposeTransaction::new(snapshot);
    let storage_before = storage_mount_identity(config).await.map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        )
    })?;

    let compose_changed = match ensure_runtime_port_mapping(&compose_path, guest_port) {
        Ok(changed) => changed,
        Err(_) => {
            return Err(runtime_error(
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimePortForwardingInvalid,
                false,
                None,
            ));
        }
    };
    if compose_changed {
        if let Err(error) = recreate_container(config).await {
            let code = if port_conflict_message(&error) {
                BackendErrorCode::RuntimePortConflict
            } else {
                BackendErrorCode::RuntimeInitializationFailed
            };
            rollback_compose(
                config,
                transaction.snapshot(),
                &storage_before,
                CapabilityId::RuntimeStart,
            )
            .await?;
            return Err(runtime_error(CapabilityId::RuntimeStart, code, true, None));
        }
        if wait_for_guest(config).await.is_err() {
            rollback_compose(
                config,
                transaction.snapshot(),
                &storage_before,
                CapabilityId::RuntimeStart,
            )
            .await?;
            return Err(runtime_error(
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimeGuestOffline,
                true,
                None,
            ));
        }
    }

    let storage_after = match storage_mount_identity(config).await {
        Ok(identity) if identity == storage_before => identity,
        _ => {
            rollback_compose(
                config,
                transaction.snapshot(),
                &storage_before,
                CapabilityId::RuntimeStart,
            )
            .await?;
            return Err(runtime_error(
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimePortForwardingInvalid,
                false,
                None,
            ));
        }
    };
    let managed_compose = read_direct_bounded(&compose_path, MAX_COMPOSE_BYTES).map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        )
    })?;
    let managed_compose_sha256 = format!("{:x}", Sha256::digest(&managed_compose));
    let binding = match runtime_host_binding(config, guest_port).await {
        Ok(binding) => binding,
        Err(_) => {
            if compose_changed {
                rollback_compose(
                    config,
                    transaction.snapshot(),
                    &storage_before,
                    CapabilityId::RuntimeStart,
                )
                .await?;
            }
            return Err(runtime_error(
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimePortForwardingInvalid,
                true,
                None,
            ));
        }
    };
    if binding.host_ip != "127.0.0.1" || binding.guest_port != guest_port {
        rollback_compose(
            config,
            transaction.snapshot(),
            &storage_before,
            CapabilityId::RuntimeStart,
        )
        .await?;
        return Err(runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        ));
    }

    let mut log_artifact = ArtifactDescriptor::create(
        &session_id,
        BackendId::LinuxWinboat,
        ArtifactKind::RuntimeLog,
    )?;
    log_artifact.media_type = Some("text/plain; charset=utf-8".to_string());
    log_artifact.location = Some(format!("mendimaru-cache://{}", log_artifact.artifact_id));
    log_artifact.backend_diagnostic_ref = Some("winboat-runtime-lifecycle".to_string());
    write_private_file(&directory.join("runtime.log"), b"").map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::OperationFailed,
            false,
            Some(log_artifact.artifact_id.clone()),
        )
    })?;
    let mut record = SessionRecord {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        session_id,
        backend: BackendId::LinuxWinboat,
        mode: RuntimeMode::StudioRunLocally,
        studio_session_id: request.studio_session_id.clone(),
        studio_state: StudioProcessState::Unknown,
        studio_process_id: None,
        state: RuntimeState::Starting,
        http_ready: false,
        host_port: binding.host_port,
        guest_port,
        started_at: Utc::now(),
        readiness_timeout_seconds: request.readiness_timeout_seconds,
        failure_code: None,
        log_artifact,
        compose_changed,
        original_compose_sha256,
        managed_compose_sha256,
        storage_mount_identity: storage_after,
    };
    append_log(
        &directory,
        "WinBoat Runtime forwarding prepared on loopback.",
    );
    write_record(&directory, &record)
        .map_err(|_| record_error(&record, CapabilityId::RuntimeStart))?;
    refresh(config, &directory, &mut record).await?;
    transaction.commit();
    Ok(status_from_record(&record))
}

pub(crate) async fn status(config: &AppConfig, session_id: &str) -> BackendResult<RuntimeStatus> {
    let (directory, mut record) = load_session(session_id, CapabilityId::RuntimeStatus)?;
    if record.state != RuntimeState::Stopped {
        refresh(config, &directory, &mut record).await?;
    }
    Ok(status_from_record(&record))
}

pub(crate) async fn wait(config: &AppConfig, session_id: &str) -> BackendResult<RuntimeStatus> {
    let (directory, mut record) = load_session(session_id, CapabilityId::RuntimeWait)?;
    if record.state == RuntimeState::Stopped {
        return Ok(status_from_record(&record));
    }
    let timeout = Duration::from_secs(record.readiness_timeout_seconds.clamp(1, 3_600));
    let started = tokio::time::Instant::now();
    while started.elapsed() < timeout {
        refresh(config, &directory, &mut record).await?;
        if record.state == RuntimeState::Ready {
            return Ok(status_from_record(&record));
        }
        if matches!(
            record.failure_code,
            Some(
                BackendErrorCode::RuntimePortConflict
                    | BackendErrorCode::RuntimePortForwardingInvalid
                    | BackendErrorCode::RuntimeComposeRecoveryFailed
                    | BackendErrorCode::RuntimeExited
            )
        ) {
            return Err(record_error(&record, CapabilityId::RuntimeWait));
        }
        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }

    let mut code = diagnose_unready_runtime(config, &record).await;
    if code == BackendErrorCode::RuntimeReadinessTimeout && record.studio_session_id.is_some() && {
        observe_studio(config, &mut record).await;
        record.studio_state == StudioProcessState::Stopped
    } {
        code = BackendErrorCode::RuntimeExited;
    }
    let previous_failure = (record.state, record.failure_code);
    record.state = RuntimeState::Failed;
    record.http_ready = false;
    record.failure_code = Some(code);
    append_failure_diagnostic(
        &directory,
        &record,
        previous_failure,
        diagnostic_log_message(code),
    );
    write_record(&directory, &record)
        .map_err(|_| record_error(&record, CapabilityId::RuntimeWait))?;
    Err(record_error(&record, CapabilityId::RuntimeWait))
}

pub(crate) async fn url(config: &AppConfig, session_id: &str) -> BackendResult<String> {
    let status = status(config, session_id).await?;
    status.url.ok_or_else(|| {
        runtime_error(
            CapabilityId::RuntimeUrl,
            status
                .failure_code
                .unwrap_or(BackendErrorCode::RuntimeReadinessTimeout),
            true,
            status
                .log_artifact
                .as_ref()
                .map(|artifact| artifact.artifact_id.clone()),
        )
    })
}

pub(crate) async fn stop(config: &AppConfig, session_id: &str) -> BackendResult<()> {
    let (directory, mut record) = load_session(session_id, CapabilityId::RuntimeStop)?;
    if record.state == RuntimeState::Stopped {
        return Ok(());
    }
    let original = read_direct_bounded(&directory.join("compose.original.yml"), MAX_COMPOSE_BYTES)
        .map_err(|_| {
            runtime_error(
                CapabilityId::RuntimeStop,
                BackendErrorCode::RuntimeComposeRecoveryFailed,
                false,
                Some(record.log_artifact.artifact_id.clone()),
            )
        })?;
    if format!("{:x}", Sha256::digest(&original)) != record.original_compose_sha256 {
        return Err(runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        ));
    }
    let compose_path = direct_compose_path(config, CapabilityId::RuntimeStop)?;
    let managed = read_direct_bounded(&compose_path, MAX_COMPOSE_BYTES).map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        )
    })?;
    let current_compose_sha256 = format!("{:x}", Sha256::digest(&managed));
    if current_compose_sha256 != record.managed_compose_sha256
        && current_compose_sha256 != record.original_compose_sha256
    {
        return Err(runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        ));
    }
    if current_compose_sha256 != record.original_compose_sha256 {
        write_atomic_compose(&compose_path, &original).map_err(|_| {
            runtime_error(
                CapabilityId::RuntimeStop,
                BackendErrorCode::RuntimeComposeRecoveryFailed,
                false,
                Some(record.log_artifact.artifact_id.clone()),
            )
        })?;
    }
    recreate_container(config).await.map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            true,
            Some(record.log_artifact.artifact_id.clone()),
        )
    })?;
    wait_for_guest(config).await.map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeGuestOffline,
            true,
            Some(record.log_artifact.artifact_id.clone()),
        )
    })?;
    let storage_after = storage_mount_identity(config).await.map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        )
    })?;
    if storage_after != record.storage_mount_identity {
        return Err(runtime_error(
            CapabilityId::RuntimeStop,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        ));
    }
    record.state = RuntimeState::Stopped;
    record.http_ready = false;
    record.failure_code = None;
    record.studio_state = StudioProcessState::Stopped;
    record.studio_process_id = None;
    append_log(
        &directory,
        "WinBoat Runtime stopped and original Compose restored.",
    );
    write_record(&directory, &record).map_err(|_| record_error(&record, CapabilityId::RuntimeStop))
}

pub(crate) fn logs(session_id: &str, cursor: Option<&str>) -> BackendResult<RuntimeLogBatch> {
    let (directory, record) = load_session(session_id, CapabilityId::RuntimeLogs)?;
    let start = cursor.unwrap_or("0").parse::<usize>().map_err(|_| {
        runtime_error(
            CapabilityId::RuntimeLogs,
            BackendErrorCode::InvalidRequest,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        )
    })?;
    let content = read_direct_bounded(&directory.join("runtime.log"), MAX_LOG_BYTES)
        .map_err(|_| record_error(&record, CapabilityId::RuntimeLogs))?;
    let text = String::from_utf8_lossy(&content);
    let lines = text.lines().collect::<Vec<_>>();
    if start > lines.len() {
        return Err(runtime_error(
            CapabilityId::RuntimeLogs,
            BackendErrorCode::InvalidRequest,
            false,
            Some(record.log_artifact.artifact_id.clone()),
        ));
    }
    let end = (start + 1_000).min(lines.len());
    Ok(RuntimeLogBatch {
        session_id: session_id.to_string(),
        entries: lines[start..end]
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
        next_cursor: (end < lines.len()).then(|| end.to_string()),
        truncated: end < lines.len(),
    })
}

async fn refresh(
    config: &AppConfig,
    directory: &Path,
    record: &mut SessionRecord,
) -> BackendResult<()> {
    if !guest_is_online(config).await {
        let previous_failure = (record.state, record.failure_code);
        record.state = RuntimeState::Failed;
        record.http_ready = false;
        record.failure_code = Some(BackendErrorCode::RuntimeGuestOffline);
        record.studio_state = StudioProcessState::Unknown;
        append_failure_diagnostic(
            directory,
            record,
            previous_failure,
            "Guest API health probe failed.",
        );
        write_record(directory, record)
            .map_err(|_| record_error(record, CapabilityId::RuntimeStatus))?;
        return Ok(());
    }
    let binding = match runtime_host_binding(config, record.guest_port).await {
        Ok(binding) => binding,
        Err(_) => {
            let code = BackendErrorCode::RuntimePortForwardingInvalid;
            let previous_failure = (record.state, record.failure_code);
            record.state = RuntimeState::Failed;
            record.http_ready = false;
            record.failure_code = Some(code);
            append_failure_diagnostic(
                directory,
                record,
                previous_failure,
                "The loopback Runtime host binding could not be inspected.",
            );
            write_record(directory, record)
                .map_err(|_| record_error(record, CapabilityId::RuntimeStatus))?;
            return Ok(());
        }
    };
    if binding.host_ip != "127.0.0.1" || binding.guest_port != record.guest_port {
        let code = BackendErrorCode::RuntimePortForwardingInvalid;
        let previous_failure = (record.state, record.failure_code);
        record.state = RuntimeState::Failed;
        record.http_ready = false;
        record.failure_code = Some(code);
        append_failure_diagnostic(
            directory,
            record,
            previous_failure,
            "The loopback Runtime host binding did not match the recorded guest port.",
        );
        write_record(directory, record)
            .map_err(|_| record_error(record, CapabilityId::RuntimeStatus))?;
        return Ok(());
    }
    if binding.host_port != record.host_port {
        record.host_port = binding.host_port;
        append_log(
            directory,
            "WinBoat guest restart changed the loopback host port.",
        );
    }

    let url = runtime_url(record.host_port);
    record.http_ready = http_ready(&url).await;
    apply_http_only_readiness(record);
    write_record(directory, record).map_err(|_| record_error(record, CapabilityId::RuntimeStatus))
}

fn apply_http_only_readiness(record: &mut SessionRecord) {
    if record.http_ready {
        record.state = RuntimeState::Ready;
        record.failure_code = None;
        record.studio_state = StudioProcessState::Running;
        return;
    }
    record.state = RuntimeState::Starting;
    record.failure_code = None;
}

async fn observe_studio(config: &AppConfig, record: &mut SessionRecord) {
    let sessions = match super::studio_sessions(config).await {
        Ok(sessions) => sessions,
        Err(_) => {
            record.studio_state = StudioProcessState::Unknown;
            record.studio_process_id = None;
            return;
        }
    };
    let selected = if let Some(session_id) = record.studio_session_id.as_deref() {
        sessions
            .into_iter()
            .find(|session| session.session_id == session_id)
    } else {
        None
    };
    if let Some(session) = selected {
        record.studio_state = session.state;
        record.studio_process_id = session.process_id;
    } else {
        record.studio_state = StudioProcessState::Stopped;
        record.studio_process_id = None;
    }
}

async fn diagnose_unready_runtime(config: &AppConfig, record: &SessionRecord) -> BackendErrorCode {
    if !guest_is_online(config).await {
        return BackendErrorCode::RuntimeGuestOffline;
    }
    if runtime_host_binding(config, record.guest_port)
        .await
        .is_err()
    {
        return BackendErrorCode::RuntimePortForwardingInvalid;
    }
    diagnostic_code(guest_port_diagnostic(config, record.guest_port).await)
}

fn diagnostic_code(diagnostic: Option<&str>) -> BackendErrorCode {
    match diagnostic {
        Some("MENDIMARU_RUNTIME_NOT_LISTENING") => BackendErrorCode::RuntimeNotListening,
        Some("MENDIMARU_RUNTIME_FIREWALL_BLOCKED") | Some("MENDIMARU_RUNTIME_LISTENING") => {
            BackendErrorCode::RuntimeFirewallBlocked
        }
        _ => BackendErrorCode::RuntimeReadinessTimeout,
    }
}

async fn guest_port_diagnostic(config: &AppConfig, guest_port: u16) -> Option<&'static str> {
    if !super::registered_client_sessions().is_empty() {
        return None;
    }
    let identifier = secure_identifier("runtimeprobe").ok()?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations").ok()?;
    let report_path = operation_directory.join(format!("{identifier}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )
    .ok()?;
    let script = runtime_port_probe_script(guest_port, &windows_report_path);
    let command = write_command_script(config, &identifier, &script).ok()?;
    let outcome = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: "Diagnose Mendix Runtime port",
            report_path: &report_path,
            timeout_seconds: PORT_DIAGNOSTIC_TIMEOUT_SECONDS,
            operation: "diagnosing the Mendix Runtime port",
            keep_remote_app_alive: false,
            cancellation: None,
            project_access: None,
        },
        |_| {},
    )
    .await;
    let _ = fs::remove_file(&command.path);
    let _ = fs::remove_file(&report_path);
    let mut temporary = report_path.as_os_str().to_os_string();
    temporary.push(".tmp");
    let _ = fs::remove_file(PathBuf::from(temporary));
    match outcome.ok()?.report.message.as_str() {
        "MENDIMARU_RUNTIME_NOT_LISTENING" => Some("MENDIMARU_RUNTIME_NOT_LISTENING"),
        "MENDIMARU_RUNTIME_FIREWALL_BLOCKED" => Some("MENDIMARU_RUNTIME_FIREWALL_BLOCKED"),
        "MENDIMARU_RUNTIME_LISTENING" => Some("MENDIMARU_RUNTIME_LISTENING"),
        _ => None,
    }
}

async fn rollback_compose(
    config: &AppConfig,
    snapshot: &crate::config::FileSnapshot,
    expected_storage: &[String],
    capability: CapabilityId,
) -> BackendResult<()> {
    let restored = restore_file(snapshot).is_ok();
    let recreated = restored && recreate_container(config).await.is_ok();
    let online = recreated && wait_for_guest(config).await.is_ok();
    let storage_matches = if online {
        storage_mount_identity(config)
            .await
            .is_ok_and(|identity| identity.as_slice() == expected_storage)
    } else {
        false
    };
    if !storage_matches {
        return Err(runtime_error(
            capability,
            BackendErrorCode::RuntimeComposeRecoveryFailed,
            false,
            None,
        ));
    }
    Ok(())
}

async fn wait_for_guest(config: &AppConfig) -> Result<(), ()> {
    let timeout = Duration::from_secs(config.startup_timeout_seconds.clamp(1, 3_600));
    let started = tokio::time::Instant::now();
    while started.elapsed() < timeout {
        if guest_is_online(config).await {
            return Ok(());
        }
        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }
    Err(())
}

async fn http_ready(url: &str) -> bool {
    let parsed = match reqwest::Url::parse(url) {
        Ok(url)
            if url.scheme() == "http"
                && url.host_str().is_some_and(is_loopback_host)
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            url
        }
        _ => return false,
    };
    let client = match reqwest::Client::builder()
        .timeout(HTTP_PROBE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    client
        .get(parsed)
        .send()
        .await
        .is_ok_and(|response| response.status().as_u16() < 500)
}

fn validate_start_request(request: &RuntimeStartRequest) -> BackendResult<()> {
    if request.mode != RuntimeMode::StudioRunLocally
        || request.package_artifact_id.is_some()
        || !(1..=3_600).contains(&request.readiness_timeout_seconds)
        || !(1024..=u16::MAX).contains(&request.guest_port.unwrap_or(DEFAULT_GUEST_PORT))
        || request
            .studio_session_id
            .as_deref()
            .is_some_and(|session_id| !valid_studio_session_id(session_id))
    {
        return Err(runtime_error(
            CapabilityId::RuntimeStart,
            BackendErrorCode::InvalidRequest,
            false,
            None,
        ));
    }
    Ok(())
}

fn valid_studio_session_id(value: &str) -> bool {
    if value.len() > 48 {
        return false;
    }
    let mut parts = value.strip_prefix("studio-").unwrap_or_default().split('-');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(pid), Some(ticks), None)
            if pid.parse::<u32>().is_ok_and(|pid| pid > 0)
                && ticks.parse::<i64>().is_ok_and(|ticks| ticks > 0)
    )
}

fn direct_compose_path(config: &AppConfig, capability: CapabilityId) -> BackendResult<PathBuf> {
    let path = PathBuf::from(&config.compose_file);
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        runtime_error(
            capability,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(runtime_error(
            capability,
            BackendErrorCode::RuntimePortForwardingInvalid,
            false,
            None,
        ));
    }
    Ok(path)
}

fn layout() -> Result<PathBuf, String> {
    let paths = AppPaths::discover_for_cli()?;
    paths.ensure_cache_directory()?;
    let root = paths.cache_directory().join(STORE_DIRECTORY);
    ensure_private_directory(&root)?;
    ensure_private_directory(&root.join("sessions"))?;
    Ok(root)
}

fn session_directory(layout: &Path, session_id: &str) -> Result<PathBuf, String> {
    validate_runtime_session_id(session_id)?;
    let directory = layout.join("sessions").join(session_id);
    ensure_private_directory(&directory)?;
    Ok(directory)
}

fn session_record_path(layout: &Path, session_id: &str) -> PathBuf {
    layout
        .join("sessions")
        .join(session_id)
        .join("session.json")
}

fn load_session(
    session_id: &str,
    capability: CapabilityId,
) -> BackendResult<(PathBuf, SessionRecord)> {
    validate_runtime_session_id(session_id).map_err(|_| {
        runtime_error(
            capability,
            BackendErrorCode::RuntimeSessionNotFound,
            false,
            None,
        )
    })?;
    let layout = layout().map_err(|_| {
        runtime_error(
            capability,
            BackendErrorCode::RuntimeSessionNotFound,
            false,
            None,
        )
    })?;
    let directory = layout.join("sessions").join(session_id);
    let record: SessionRecord =
        read_json_bounded(&directory.join("session.json"), MAX_RECORD_BYTES).map_err(|_| {
            runtime_error(
                capability,
                BackendErrorCode::RuntimeSessionNotFound,
                false,
                None,
            )
        })?;
    if record.schema_version != CONTRACT_SCHEMA_VERSION
        || record.session_id != session_id
        || record.backend != BackendId::LinuxWinboat
        || record.mode != RuntimeMode::StudioRunLocally
    {
        return Err(runtime_error(
            capability,
            BackendErrorCode::RuntimeSessionNotFound,
            false,
            None,
        ));
    }
    Ok((directory, record))
}

fn write_record(directory: &Path, record: &SessionRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| format!("could not serialize WinBoat Runtime state: {error}"))?;
    write_atomic_private(&directory.join("session.json"), &bytes)
}

fn status_from_record(record: &SessionRecord) -> RuntimeStatus {
    RuntimeStatus {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        session_id: record.session_id.clone(),
        backend: record.backend,
        mode: record.mode,
        runtime_version: None,
        state: record.state,
        process_id: record.studio_process_id,
        started_at: Some(record.started_at),
        url: record.http_ready.then(|| runtime_url(record.host_port)),
        host_port: Some(record.host_port),
        guest_port: Some(record.guest_port),
        studio_session_id: record.studio_session_id.clone(),
        studio_state: Some(record.studio_state),
        http_ready: record.http_ready,
        failure_code: record.failure_code,
        log_artifact: Some(record.log_artifact.clone()),
    }
}

fn record_error(record: &SessionRecord, capability: CapabilityId) -> BackendError {
    runtime_error(
        capability,
        record
            .failure_code
            .unwrap_or(BackendErrorCode::OperationFailed),
        matches!(
            record.failure_code,
            Some(
                BackendErrorCode::RuntimeGuestOffline
                    | BackendErrorCode::RuntimePortConflict
                    | BackendErrorCode::RuntimePortForwardingInvalid
                    | BackendErrorCode::RuntimeFirewallBlocked
                    | BackendErrorCode::RuntimeNotListening
                    | BackendErrorCode::RuntimeReadinessTimeout
            )
        ),
        Some(record.log_artifact.artifact_id.clone()),
    )
}

fn runtime_error(
    capability: CapabilityId,
    code: BackendErrorCode,
    retryable: bool,
    diagnostic_ref: Option<String>,
) -> BackendError {
    BackendError {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        code,
        message: error_message(code).to_string(),
        backend: Some(BackendId::LinuxWinboat),
        capability: Some(capability),
        reason: None,
        retryable,
        diagnostic_ref,
    }
}

fn error_message(code: BackendErrorCode) -> &'static str {
    match code {
        BackendErrorCode::RuntimeGuestOffline => "the WinBoat guest is offline",
        BackendErrorCode::RuntimePortConflict => "the WinBoat Runtime host port conflicts",
        BackendErrorCode::RuntimePortForwardingInvalid => {
            "the WinBoat Runtime port forwarding is invalid"
        }
        BackendErrorCode::RuntimeFirewallBlocked => {
            "the Windows firewall or Mendix port security blocks the Runtime port"
        }
        BackendErrorCode::RuntimeNotListening => {
            "the Mendix Runtime is not listening inside the WinBoat guest"
        }
        BackendErrorCode::RuntimeComposeRecoveryFailed => {
            "the original WinBoat Compose configuration could not be recovered"
        }
        BackendErrorCode::RuntimeReadinessTimeout => {
            "the WinBoat Runtime did not become HTTP-ready before the timeout"
        }
        BackendErrorCode::RuntimeSessionNotFound => "the WinBoat Runtime session was not found",
        BackendErrorCode::RuntimeExited => "the linked Studio Pro session ended",
        BackendErrorCode::RuntimeInitializationFailed => {
            "the WinBoat Runtime forwarding failed during initialization"
        }
        BackendErrorCode::InvalidRequest => "the WinBoat Runtime request is invalid",
        BackendErrorCode::PreconditionFailed => "a WinBoat Runtime precondition was not satisfied",
        BackendErrorCode::OperationFailed => "the WinBoat Runtime operation could not be completed",
        _ => "the WinBoat Runtime operation could not be completed",
    }
}

fn diagnostic_log_message(code: BackendErrorCode) -> &'static str {
    match code {
        BackendErrorCode::RuntimeGuestOffline => "Runtime wait failed: WinBoat guest offline.",
        BackendErrorCode::RuntimePortForwardingInvalid => {
            "Runtime wait failed: loopback forwarding missing or invalid."
        }
        BackendErrorCode::RuntimeFirewallBlocked => {
            "Runtime wait failed: Windows firewall or Mendix port security blocked access."
        }
        BackendErrorCode::RuntimeNotListening => {
            "Runtime wait failed: no guest Runtime listener was detected."
        }
        _ => "Runtime wait failed: HTTP readiness timeout.",
    }
}

fn runtime_url(host_port: u16) -> String {
    format!("http://127.0.0.1:{host_port}")
}

fn port_conflict_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "address already in use",
        "port is already allocated",
        "bind for",
        "port conflict",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

fn is_loopback_host(value: &str) -> bool {
    value.eq_ignore_ascii_case("localhost")
        || value
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_runtime_session_id(value: &str) -> Result<(), String> {
    let suffix = value
        .strip_prefix("runtime_")
        .ok_or_else(|| "invalid Runtime session identifier".to_string())?;
    if suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("invalid Runtime session identifier".to_string())
    }
}

fn read_direct_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect private file: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_bytes {
        return Err("the private file is invalid or exceeds its limit".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(max_bytes + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("could not read private file: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err("the private file exceeds its limit".to_string());
    }
    Ok(bytes)
}

fn read_json_bounded<T: for<'de> Deserialize<'de>>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, String> {
    serde_json::from_slice(&read_direct_bounded(path, max_bytes)?)
        .map_err(|error| format!("could not parse private state: {error}"))
}

fn write_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the private file has no parent".to_string())?;
    ensure_private_directory(parent)?;
    if path.exists() {
        return Err("the private file already exists".to_string());
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("could not create private file: {error}"))?;
    set_file_permissions(path)?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not persist private file: {error}"))
}

fn write_atomic_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the private file has no parent".to_string())?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("could not inspect private file parent: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the private file parent is invalid".to_string());
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("the private file target is invalid".to_string());
        }
    }
    let nonce = secure_identifier("tmp")
        .map_err(|error| format!("could not create private file nonce: {error}"))?;
    let temporary = parent.join(format!(".{nonce}.tmp"));
    write_private_file(&temporary, content)?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("could not replace private file: {error}")
    })
}

fn write_atomic_compose(path: &Path, content: &[u8]) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect Compose file: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("the Compose target is not a direct file".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "the Compose file has no parent".to_string())?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("could not inspect Compose parent: {error}"))?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err("the Compose parent is not a direct directory".to_string());
    }
    let nonce = secure_identifier("tmp")
        .map_err(|error| format!("could not create Compose file nonce: {error}"))?;
    let temporary = parent.join(format!(".{nonce}.compose.tmp"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("could not create temporary Compose file: {error}"))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not persist temporary Compose file: {error}"))?;
    fs::set_permissions(&temporary, metadata.permissions())
        .map_err(|error| format!("could not protect temporary Compose file: {error}"))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("could not restore Compose file: {error}")
    })
}

fn append_log(directory: &Path, message: &str) {
    if message.contains('\n') || message.contains('\r') || message.len() > 512 {
        return;
    }
    let path = directory.join("runtime.log");
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() >= MAX_LOG_BYTES {
        return;
    }
    if let Ok(mut file) = OpenOptions::new().append(true).open(path) {
        let _ = writeln!(file, "{} {message}", Utc::now().to_rfc3339());
    }
}

fn append_failure_diagnostic(
    directory: &Path,
    record: &SessionRecord,
    previous_failure: (RuntimeState, Option<BackendErrorCode>),
    reason: &str,
) {
    let Some(code) = record.failure_code else {
        return;
    };
    if previous_failure == (RuntimeState::Failed, Some(code)) {
        return;
    }
    let code = serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let studio_session = record.studio_session_id.as_deref().unwrap_or("unspecified");
    let studio_process = record
        .studio_process_id
        .map_or_else(|| "unspecified".to_string(), |pid| pid.to_string());
    let message = format!(
        "Runtime failure recorded: code={code}; reason={reason}; httpReady={}; \
         studioSession={studio_session}; studioState={:?}; studioProcessId={studio_process}.",
        record.http_ready, record.studio_state
    );
    append_log(directory, &message);
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("could not create private directory: {error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect private directory: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the private directory is invalid".to_string());
    }
    set_directory_permissions(path)
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not protect private directory: {error}"))
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not protect private file: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        append_failure_diagnostic, diagnostic_code, is_loopback_host, port_conflict_message,
        runtime_url, valid_studio_session_id, validate_runtime_session_id, ComposeTransaction,
        SessionRecord,
    };
    use crate::contracts::{
        ArtifactKind, BackendErrorCode, BackendId, RuntimeMode, RuntimeState, StudioProcessState,
    };
    #[cfg(unix)]
    use crate::models::{AppConfig, ContainerRuntime};
    #[cfg(unix)]
    use crate::process::CommandPolicy;
    use chrono::Utc;
    #[cfg(unix)]
    use std::time::Duration;

    #[test]
    fn validates_only_opaque_runtime_and_exact_studio_process_identities() {
        assert!(validate_runtime_session_id(&format!("runtime_{}", "ab".repeat(16))).is_ok());
        assert!(validate_runtime_session_id("runtime_../compose").is_err());
        assert!(valid_studio_session_id("studio-4242-638908128000000000"));
        assert!(!valid_studio_session_id("studio-0-638908128000000000"));
        assert!(!valid_studio_session_id("studio-4242-1-extra"));
    }

    #[test]
    fn runtime_failure_diagnostics_are_complete_and_transition_deduplicated() {
        let temporary = tempfile::tempdir().expect("temporary Runtime directory");
        let log = temporary.path().join("runtime.log");
        std::fs::write(&log, "2026-09-01T00:00:00Z initialized\n").expect("initial log");
        let mut record = SessionRecord {
            schema_version: super::CONTRACT_SCHEMA_VERSION.to_string(),
            session_id: format!("runtime_{}", "a".repeat(32)),
            backend: BackendId::LinuxWinboat,
            mode: RuntimeMode::StudioRunLocally,
            studio_session_id: Some("studio-4242-638908128000000000".into()),
            studio_state: StudioProcessState::Stopped,
            studio_process_id: None,
            state: RuntimeState::Starting,
            http_ready: false,
            host_port: 49152,
            guest_port: 8080,
            started_at: Utc::now(),
            readiness_timeout_seconds: 5,
            failure_code: None,
            log_artifact: crate::contracts::ArtifactDescriptor::create(
                "session",
                BackendId::LinuxWinboat,
                ArtifactKind::RuntimeLog,
            )
            .expect("log artifact"),
            compose_changed: true,
            original_compose_sha256: "a".repeat(64),
            managed_compose_sha256: "b".repeat(64),
            storage_mount_identity: vec!["fixture-storage".into()],
        };
        record.failure_code = Some(BackendErrorCode::RuntimeExited);

        append_failure_diagnostic(
            temporary.path(),
            &record,
            (record.state, record.failure_code),
            "The linked Studio Pro session was absent from the authoritative session report.",
        );
        record.state = RuntimeState::Failed;
        record.failure_code = Some(BackendErrorCode::RuntimeExited);
        append_failure_diagnostic(
            temporary.path(),
            &record,
            (record.state, record.failure_code),
            "The linked Studio Pro session was absent from the authoritative session report.",
        );

        let content = std::fs::read_to_string(log).expect("Runtime diagnostic log");
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains("code=runtime_exited"));
        assert!(content.contains("httpReady=false"));
        assert!(content.contains("studioSession=studio-4242-638908128000000000"));
        assert!(content.contains("studioState=Stopped"));
        assert!(content.contains("studioProcessId=unspecified"));
    }

    #[test]
    fn exposes_only_ipv4_loopback_urls_and_classifies_runtime_port_conflicts() {
        assert_eq!(runtime_url(49152), "http://127.0.0.1:49152");
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(port_conflict_message(
            "Bind for 0.0.0.0 failed: port is already allocated"
        ));
        assert!(!port_conflict_message("guest server is offline"));
    }

    #[test]
    fn maps_guest_listener_and_firewall_diagnostics_to_distinct_codes() {
        assert_eq!(
            diagnostic_code(Some("MENDIMARU_RUNTIME_NOT_LISTENING")),
            BackendErrorCode::RuntimeNotListening
        );
        assert_eq!(
            diagnostic_code(Some("MENDIMARU_RUNTIME_FIREWALL_BLOCKED")),
            BackendErrorCode::RuntimeFirewallBlocked
        );
        assert_eq!(
            diagnostic_code(Some("MENDIMARU_RUNTIME_LISTENING")),
            BackendErrorCode::RuntimeFirewallBlocked
        );
        assert_eq!(
            diagnostic_code(None),
            BackendErrorCode::RuntimeReadinessTimeout
        );
    }

    #[test]
    fn cancelled_compose_transaction_restores_the_exact_original_bytes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let compose = temporary.path().join("docker-compose.yml");
        let original = b"services:\n  windows:\n    image: original\n";
        std::fs::write(&compose, original).expect("original Compose");
        let snapshot = crate::config::snapshot_file(&compose).expect("Compose snapshot");
        let transaction = ComposeTransaction::new(snapshot);
        std::fs::write(&compose, b"services:\n  windows:\n    image: changed\n")
            .expect("changed Compose");
        drop(transaction);
        assert_eq!(std::fs::read(compose).expect("restored Compose"), original);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_compose_start_and_recreate_restore_the_original_bytes() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let compose = temporary.path().join("docker-compose.yml");
        let runtime = temporary.path().join("fake-docker");
        let original = b"services:\n  windows:\n    image: original\n";
        std::fs::write(&compose, original).expect("original Compose");
        std::fs::write(
            &runtime,
            "#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n",
        )
        .expect("fake runtime");
        let mut permissions = std::fs::metadata(&runtime)
            .expect("runtime metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&runtime, permissions).expect("executable runtime");
        let config = AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: compose.to_string_lossy().to_string(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47271".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47273,
            shared_directory: temporary.path().to_string_lossy().to_string(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        };

        for force_recreate in [false, true] {
            let snapshot = crate::config::snapshot_file(&compose).expect("Compose snapshot");
            let transaction = ComposeTransaction::new(snapshot);
            std::fs::write(&compose, b"services:\n  windows:\n    image: changed\n")
                .expect("changed Compose");
            let result = super::super::container::compose_up_with_policy(
                &config,
                force_recreate,
                "windows",
                runtime.to_str().expect("runtime path"),
                CommandPolicy::new(Duration::from_millis(100), 1024),
            )
            .await;
            assert!(result.is_err(), "hanging Compose command must time out");
            drop(transaction);
            assert_eq!(std::fs::read(&compose).expect("restored Compose"), original);
        }
    }
}
