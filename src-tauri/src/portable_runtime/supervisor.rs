use super::process::{self, ProcessIdentity, RuntimeContainment};
use super::store::{self, RuntimeLayout};
use super::{runtime_error, ArtifactRecord, ArtifactRole, BuildRecord};
use crate::contracts::{
    secure_identifier, ArtifactDescriptor, ArtifactKind, BackendError, BackendErrorCode, BackendId,
    BackendResult, CapabilityId, RuntimeLogBatch, RuntimeMode, RuntimeStartRequest, RuntimeState,
    RuntimeStatus, CONTRACT_SCHEMA_VERSION,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use zeroize::Zeroize;

const ENVIRONMENT_JSON: &str = "MENDIMARU_RUNTIME_ENV_JSON";
const MAX_ENVIRONMENT_BYTES: usize = 64 * 1024;
const MAX_ENVIRONMENT_VALUES: usize = 64;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 8 * 1024;
const MAX_HANDSHAKE_BYTES: usize = 128 * 1024;
const MAX_LOG_BYTES: u64 = 128 * 1024 * 1024;
const MAX_LOG_BATCH_BYTES: u64 = 64 * 1024;
const SUPERVISOR_ACK_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    schema_version: String,
    session_id: String,
    backend: BackendId,
    artifact_id: String,
    state: RuntimeState,
    failure_code: Option<BackendErrorCode>,
    supervisor_pid: Option<u32>,
    supervisor_start_token: Option<String>,
    runtime_pid: Option<u32>,
    runtime_start_token: Option<String>,
    runtime_port: u16,
    admin_port: u16,
    url: String,
    started_at: Option<DateTime<Utc>>,
    readiness_timeout_seconds: u64,
    log_artifact: ArtifactDescriptor,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorPayload {
    admin_password: String,
    environment: BTreeMap<String, String>,
    java_home: String,
    java_executable: String,
}

impl Drop for SupervisorPayload {
    fn drop(&mut self) {
        self.admin_password.zeroize();
        for value in self.environment.values_mut() {
            value.zeroize();
        }
        self.java_home.zeroize();
        self.java_executable.zeroize();
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<RuntimeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BackendError>,
}

struct SupervisorGuard {
    identity: ProcessIdentity,
    armed: bool,
}

impl SupervisorGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        if self.armed {
            process::terminate_supervisor(&self.identity);
        }
    }
}

struct SecretRedactor {
    patterns: Vec<String>,
}

impl SecretRedactor {
    fn new(payload: &SupervisorPayload, protected_values: &[String]) -> Self {
        let mut patterns = payload
            .environment
            .values()
            .chain(std::iter::once(&payload.admin_password))
            .chain(std::iter::once(&payload.java_home))
            .chain(std::iter::once(&payload.java_executable))
            .chain(protected_values.iter())
            .filter(|value| value.len() >= 3)
            .flat_map(|value| {
                [
                    value.clone(),
                    base64::engine::general_purpose::STANDARD.encode(value.as_bytes()),
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.as_bytes()),
                ]
            })
            .collect::<Vec<_>>();
        patterns.sort_by_key(|value| std::cmp::Reverse(value.len()));
        patterns.dedup();
        Self { patterns }
    }

    fn redact(&self, value: &str) -> String {
        let mut redacted = value.to_string();
        for pattern in &self.patterns {
            redacted = redacted.replace(pattern, "[REDACTED]");
        }
        redacted
            .chars()
            .map(|character| {
                if character == '\t' || !character.is_control() {
                    character
                } else {
                    ' '
                }
            })
            .collect()
    }
}

struct LogSink {
    file: tokio::fs::File,
    written: u64,
    saturated: bool,
}

pub(super) async fn start(
    request: &RuntimeStartRequest,
    backend: BackendId,
) -> BackendResult<RuntimeStatus> {
    if request.mode != RuntimeMode::Portable {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        ));
    }
    if !(1..=3_600).contains(&request.readiness_timeout_seconds) {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        ));
    }
    let artifact_id = request.package_artifact_id.as_deref().ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        )
    })?;
    let layout = RuntimeLayout::discover().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        )
    })?;
    let artifact: ArtifactRecord =
        store::read_json(&layout.artifact_record(artifact_id).map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::InvalidRequest,
                None,
                false,
            )
        })?)
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::PreconditionFailed,
                None,
                false,
            )
        })?;
    if artifact.role != ArtifactRole::Package
        || artifact.descriptor.kind != ArtifactKind::RuntimePackage
        || artifact.descriptor.backend != backend
    {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        ));
    }
    let build_directory = layout
        .build_directory(&artifact.project_key, &artifact.build_key)
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::PreconditionFailed,
                None,
                false,
            )
        })?;
    let build: BuildRecord =
        store::read_json(&build_directory.join("build.json")).map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::PreconditionFailed,
                None,
                false,
            )
        })?;
    if !build.success
        || build.package_artifact.as_ref() != Some(&artifact.descriptor)
        || build.project_key != artifact.project_key
        || build.build_key != artifact.build_key
    {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        ));
    }
    let java_home = validate_private_absolute_path(&build.java_home, true).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::ToolchainUnavailable,
            None,
            false,
        )
    })?;
    let java_executable =
        validate_private_absolute_path(&build.java_executable, false).map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::ToolchainUnavailable,
                None,
                false,
            )
        })?;

    let session_id = secure_identifier("runtime")?;
    let session_directory = layout.session_directory(&session_id).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::OperationFailed,
            None,
            true,
        )
    })?;
    let source_template = build_directory.join("deployment-template");
    let deployment = session_directory.join("deployment");
    let source_for_copy = source_template.clone();
    let deployment_for_copy = deployment.clone();
    tokio::task::spawn_blocking(move || copy_deployment(&source_for_copy, &deployment_for_copy))
        .await
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimeInitializationFailed,
                None,
                true,
            )
        })?
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimeInitializationFailed,
                None,
                false,
            )
        })?;
    let runtime_port = available_loopback_port().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::PreconditionFailed,
            None,
            true,
        )
    })?;
    let admin_port = loop {
        let candidate = available_loopback_port().map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::PreconditionFailed,
                None,
                true,
            )
        })?;
        if candidate != runtime_port {
            break candidate;
        }
    };
    let url = format!("http://127.0.0.1:{runtime_port}");
    let override_path = session_directory.join("runtime-overrides.json");
    store::write_json(
        &override_path,
        &serde_json::json!({
            "admin": {
                "port": admin_port,
                "addresses": ["127.0.0.1"],
            },
            "runtime": {
                "http": {
                    "port": runtime_port,
                    "addresses": ["127.0.0.1"],
                },
                "params": {
                    "ApplicationRootUrl": format!("{url}/"),
                },
            },
        }),
    )
    .map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            None,
            false,
        )
    })?;
    let log_path = session_directory.join("runtime.log");
    store::create_private_file(&log_path, true).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            None,
            false,
        )
    })?;
    let mut log_artifact =
        ArtifactDescriptor::create(&session_id, backend, ArtifactKind::RuntimeLog)?;
    log_artifact.media_type = Some("text/plain; charset=utf-8".to_string());
    log_artifact.location = Some(format!("mendimaru-cache://{}", log_artifact.artifact_id));
    log_artifact.backend_diagnostic_ref = Some("portable-runtime-output".to_string());
    let mut record = SessionRecord {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        session_id: session_id.clone(),
        backend,
        artifact_id: artifact_id.to_string(),
        state: RuntimeState::Starting,
        failure_code: None,
        supervisor_pid: None,
        supervisor_start_token: None,
        runtime_pid: None,
        runtime_start_token: None,
        runtime_port,
        admin_port,
        url,
        started_at: None,
        readiness_timeout_seconds: request.readiness_timeout_seconds,
        log_artifact,
    };
    write_session(&layout, &record).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            None,
            true,
        )
    })?;
    let environment = runtime_environment().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        )
    })?;
    let payload = SupervisorPayload {
        admin_password: random_secret().map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimeInitializationFailed,
                None,
                true,
            )
        })?,
        environment,
        java_home: java_home.to_string_lossy().to_string(),
        java_executable: java_executable.to_string_lossy().to_string(),
    };
    let executable = std::env::current_exe().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            None,
            true,
        )
    })?;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("__runtime-supervisor")
        .arg("--session-id")
        .arg(&session_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(false)
        .env_remove(ENVIRONMENT_JSON);
    process::configure_detached_supervisor(&mut command);
    let mut child = command.spawn().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let supervisor_pid = child.id().ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let supervisor_identity = wait_for_identity(supervisor_pid).await.map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let mut guard = SupervisorGuard {
        identity: supervisor_identity.clone(),
        armed: true,
    };
    record.supervisor_pid = Some(supervisor_pid);
    record.supervisor_start_token = Some(supervisor_identity.start_token);
    write_session(&layout, &record).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let mut serialized = serde_json::to_string(&payload).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            false,
        )
    })?;
    serialized.push('\n');
    let write_result = stdin.write_all(serialized.as_bytes()).await;
    serialized.zeroize();
    write_result.map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    stdin.flush().await.map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let count = reader.read_line(&mut line).await.map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    if count == 0 || line.len() > MAX_HANDSHAKE_BYTES {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        ));
    }
    let response: SupervisorResponse = serde_json::from_str(&line).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            false,
        )
    })?;
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| {
            runtime_error(
                backend,
                CapabilityId::RuntimeStart,
                BackendErrorCode::RuntimeInitializationFailed,
                Some(record.log_artifact.artifact_id.clone()),
                false,
            )
        }));
    }
    let status = response.status.ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            false,
        )
    })?;
    stdin.write_all(b"accept\n").await.map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    stdin.flush().await.map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    line.clear();
    let confirmed = tokio::time::timeout(SUPERVISOR_ACK_TIMEOUT, reader.read_line(&mut line))
        .await
        .ok()
        .and_then(Result::ok)
        .is_some_and(|count| count > 0 && line.trim_end() == "accepted");
    if !confirmed {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeStart,
            BackendErrorCode::RuntimeInitializationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        ));
    }
    guard.disarm();
    Ok(status)
}

pub(super) async fn status(session_id: &str, backend: BackendId) -> BackendResult<RuntimeStatus> {
    let layout = RuntimeLayout::discover().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStatus,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        )
    })?;
    let mut record = load_session(&layout, session_id, backend, CapabilityId::RuntimeStatus)?;
    if matches!(
        record.state,
        RuntimeState::Starting | RuntimeState::Running | RuntimeState::Ready
    ) && !session_supervisor_alive(&record)
    {
        if let Some(identity) = runtime_identity(&record) {
            process::terminate_runtime_group(&identity, true);
        }
        record.state = RuntimeState::Failed;
        record.failure_code = Some(BackendErrorCode::RuntimeExited);
        let _ = write_session(&layout, &record);
    }
    let mut status = status_from_record(&record);
    if record.state == RuntimeState::Ready && !http_ready(&record.url, record.admin_port).await {
        status.state = RuntimeState::Running;
        status.url = None;
    }
    Ok(status)
}

pub(super) async fn wait(session_id: &str, backend: BackendId) -> BackendResult<RuntimeStatus> {
    loop {
        let status = status(session_id, backend).await?;
        if matches!(
            status.state,
            RuntimeState::Ready | RuntimeState::Failed | RuntimeState::Stopped
        ) {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub(super) async fn url(session_id: &str, backend: BackendId) -> BackendResult<String> {
    let status = status(session_id, backend).await?;
    if status.state != RuntimeState::Ready {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeUrl,
            status
                .failure_code
                .unwrap_or(BackendErrorCode::PreconditionFailed),
            status
                .log_artifact
                .as_ref()
                .map(|artifact| artifact.artifact_id.clone()),
            false,
        ));
    }
    status.url.ok_or_else(|| {
        runtime_error(
            backend,
            CapabilityId::RuntimeUrl,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        )
    })
}

pub(super) async fn stop(session_id: &str, backend: BackendId) -> BackendResult<()> {
    let layout = RuntimeLayout::discover().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStop,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        )
    })?;
    let mut record = load_session(&layout, session_id, backend, CapabilityId::RuntimeStop)?;
    if matches!(record.state, RuntimeState::Stopped | RuntimeState::Failed) {
        return Ok(());
    }
    let directory = layout.session_directory(session_id).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStop,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        )
    })?;
    store::create_private_file(&directory.join("stop.request"), true).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStop,
            BackendErrorCode::OperationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let deadline = tokio::time::Instant::now() + STOP_GRACE_PERIOD + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        record = load_session(&layout, session_id, backend, CapabilityId::RuntimeStop)?;
        if record.state == RuntimeState::Stopped {
            return Ok(());
        }
        if !session_supervisor_alive(&record) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if let Some(identity) = supervisor_identity(&record) {
        process::terminate_supervisor(&identity);
    }
    if let Some(identity) = runtime_identity(&record) {
        process::terminate_runtime_group(&identity, true);
    }
    record.state = RuntimeState::Stopped;
    record.failure_code = None;
    write_session(&layout, &record).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeStop,
            BackendErrorCode::OperationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })
}

pub(super) async fn logs(
    session_id: &str,
    cursor: Option<&str>,
    backend: BackendId,
) -> BackendResult<RuntimeLogBatch> {
    let layout = RuntimeLayout::discover().map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeLogs,
            BackendErrorCode::PreconditionFailed,
            None,
            false,
        )
    })?;
    let record = load_session(&layout, session_id, backend, CapabilityId::RuntimeLogs)?;
    let offset = cursor
        .unwrap_or("0")
        .parse::<u64>()
        .ok()
        .filter(|value| *value <= MAX_LOG_BYTES)
        .ok_or_else(|| {
            runtime_error(
                backend,
                CapabilityId::RuntimeLogs,
                BackendErrorCode::InvalidRequest,
                None,
                false,
            )
        })?;
    let path = layout
        .session_directory(session_id)
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeLogs,
                BackendErrorCode::InvalidRequest,
                None,
                false,
            )
        })?
        .join("runtime.log");
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeLogs,
            BackendErrorCode::OperationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || offset > metadata.len() {
        return Err(runtime_error(
            backend,
            CapabilityId::RuntimeLogs,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        ));
    }
    let mut file = File::open(path).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeLogs,
            BackendErrorCode::OperationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    file.seek(SeekFrom::Start(offset)).map_err(|_| {
        runtime_error(
            backend,
            CapabilityId::RuntimeLogs,
            BackendErrorCode::OperationFailed,
            Some(record.log_artifact.artifact_id.clone()),
            true,
        )
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BATCH_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            runtime_error(
                backend,
                CapabilityId::RuntimeLogs,
                BackendErrorCode::OperationFailed,
                Some(record.log_artifact.artifact_id.clone()),
                true,
            )
        })?;
    while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
        bytes.pop();
    }
    let next = offset + bytes.len() as u64;
    let truncated = next < metadata.len();
    Ok(RuntimeLogBatch {
        session_id: session_id.to_string(),
        entries: String::from_utf8(bytes)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect(),
        next_cursor: truncated.then(|| next.to_string()),
        truncated,
    })
}

pub(super) fn dispatch(arguments: &[std::ffi::OsString]) -> i32 {
    let values = arguments
        .iter()
        .map(|value| value.to_str())
        .collect::<Option<Vec<_>>>();
    let session_id = match values.as_deref() {
        Some(["--session-id", value]) => *value,
        Some([value]) if value.starts_with("--session-id=") => {
            value.trim_start_matches("--session-id=")
        }
        _ => return 2,
    };
    if store::validate_identifier(session_id, "runtime").is_err() {
        return 2;
    }
    match tauri::async_runtime::block_on(supervisor_main(session_id)) {
        Ok(()) => 0,
        Err(()) => 1,
    }
}

async fn supervisor_main(session_id: &str) -> Result<(), ()> {
    let layout = RuntimeLayout::discover().map_err(|_| ())?;
    let mut record: SessionRecord =
        store::read_json(&layout.session_record(session_id).map_err(|_| ())?).map_err(|_| ())?;
    if record.session_id != session_id
        || record.supervisor_pid != Some(std::process::id())
        || !session_supervisor_alive(&record)
    {
        return Err(());
    }
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut payload_line = String::new();
    let count = stdin.read_line(&mut payload_line).await.map_err(|_| ())?;
    if count == 0 || payload_line.len() > MAX_HANDSHAKE_BYTES {
        return Err(());
    }
    let payload: SupervisorPayload = serde_json::from_str(&payload_line).map_err(|_| ())?;
    payload_line.zeroize();
    validate_payload(&payload).map_err(|_| ())?;
    let directory = layout.session_directory(session_id).map_err(|_| ())?;
    let deployment = directory.join("deployment");
    let temp_directory = directory.join("tmp");
    store::ensure_private_directory(&temp_directory).map_err(|_| ())?;
    let protected_paths = vec![
        directory.to_string_lossy().to_string(),
        deployment.to_string_lossy().to_string(),
    ];
    let redactor = Arc::new(SecretRedactor::new(&payload, &protected_paths));
    let override_path = directory.join("runtime-overrides.json");
    let log_file =
        store::create_private_file(&directory.join("runtime.log"), false).map_err(|_| ())?;
    let log = Arc::new(Mutex::new(LogSink {
        file: tokio::fs::File::from_std(log_file),
        written: 0,
        saturated: false,
    }));
    let mut command = match runtime_command(&deployment, &override_path, &temp_directory, &payload)
    {
        Ok(command) => command,
        Err(_) => {
            append_internal_log(&log, "runtime command validation failed").await;
            fail_and_respond(
                &layout,
                &mut record,
                BackendErrorCode::RuntimeInitializationFailed,
            )
            .await;
            return Err(());
        }
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            append_internal_log(&log, "runtime process spawn failed").await;
            fail_and_respond(
                &layout,
                &mut record,
                BackendErrorCode::RuntimeInitializationFailed,
            )
            .await;
            return Err(());
        }
    };
    let runtime_pid = match child.id() {
        Some(pid) => pid,
        None => {
            append_internal_log(&log, "runtime process identity was unavailable").await;
            fail_and_respond(
                &layout,
                &mut record,
                BackendErrorCode::RuntimeInitializationFailed,
            )
            .await;
            return Err(());
        }
    };
    let containment = match RuntimeContainment::attach(runtime_pid) {
        Ok(containment) => containment,
        Err(_) => {
            let _ = child.start_kill();
            append_internal_log(&log, "runtime containment initialization failed").await;
            fail_and_respond(
                &layout,
                &mut record,
                BackendErrorCode::RuntimeInitializationFailed,
            )
            .await;
            return Err(());
        }
    };
    let runtime_identity = match wait_for_identity(runtime_pid).await {
        Ok(identity) => identity,
        Err(_) => {
            let _ = child.start_kill();
            append_internal_log(&log, "runtime process identity validation failed").await;
            fail_and_respond(
                &layout,
                &mut record,
                BackendErrorCode::RuntimeInitializationFailed,
            )
            .await;
            return Err(());
        }
    };
    record.runtime_pid = Some(runtime_identity.pid);
    record.runtime_start_token = Some(runtime_identity.start_token.clone());
    record.started_at = Some(Utc::now());
    record.state = RuntimeState::Running;
    if write_session(&layout, &record).is_err() {
        process::terminate_runtime_group(&runtime_identity, true);
        return Err(());
    }
    let stdout_task = child
        .stdout
        .take()
        .map(|stream| tokio::spawn(capture_log(stream, "stdout", log.clone(), redactor.clone())));
    let stderr_task = child
        .stderr
        .take()
        .map(|stream| tokio::spawn(capture_log(stream, "stderr", log.clone(), redactor.clone())));

    let mut acknowledgement = String::new();
    let acknowledgement_future = stdin.read_line(&mut acknowledgement);
    tokio::pin!(acknowledgement_future);
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(record.readiness_timeout_seconds);
    let failure = loop {
        tokio::select! {
            parent = &mut acknowledgement_future => {
                if parent.ok().filter(|count| *count > 0).is_none() {
                    break Some(BackendErrorCode::RuntimeInitializationFailed);
                }
                break Some(BackendErrorCode::RuntimeInitializationFailed);
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if child.try_wait().ok().flatten().is_some() {
                    break Some(BackendErrorCode::RuntimeInitializationFailed);
                }
                if http_ready(&record.url, record.admin_port).await {
                    break None;
                }
                if tokio::time::Instant::now() >= deadline {
                    break Some(BackendErrorCode::RuntimeReadinessTimeout);
                }
            }
        }
    };
    if let Some(code) = failure {
        process::terminate_runtime_group(&runtime_identity, false);
        wait_then_force(&mut child, &runtime_identity).await;
        record.state = RuntimeState::Failed;
        record.failure_code = Some(code);
        let _ = write_session(&layout, &record);
        let _ = write_supervisor_response(&SupervisorResponse {
            ok: false,
            status: None,
            error: Some(runtime_error(
                record.backend,
                CapabilityId::RuntimeStart,
                code,
                Some(record.log_artifact.artifact_id.clone()),
                code == BackendErrorCode::RuntimeReadinessTimeout,
            )),
        })
        .await;
        drop(containment);
        join_log_tasks(stdout_task, stderr_task).await;
        return Err(());
    }
    record.state = RuntimeState::Ready;
    record.failure_code = None;
    if write_session(&layout, &record).is_err() {
        process::terminate_runtime_group(&runtime_identity, true);
        return Err(());
    }
    if write_supervisor_response(&SupervisorResponse {
        ok: true,
        status: Some(status_from_record(&record)),
        error: None,
    })
    .await
    .is_err()
    {
        process::terminate_runtime_group(&runtime_identity, true);
        return Err(());
    }
    let accepted = tokio::time::timeout(SUPERVISOR_ACK_TIMEOUT, &mut acknowledgement_future)
        .await
        .ok()
        .and_then(Result::ok)
        .is_some_and(|count| count > 0 && acknowledgement.trim_end() == "accept");
    if !accepted {
        process::terminate_runtime_group(&runtime_identity, false);
        wait_then_force(&mut child, &runtime_identity).await;
        record.state = RuntimeState::Stopped;
        record.failure_code = None;
        let _ = write_session(&layout, &record);
        drop(containment);
        join_log_tasks(stdout_task, stderr_task).await;
        return Err(());
    }
    let mut stdout = tokio::io::stdout();
    if stdout.write_all(b"accepted\n").await.is_err() || stdout.flush().await.is_err() {
        process::terminate_runtime_group(&runtime_identity, true);
        return Err(());
    }

    monitor_runtime(
        &layout,
        &directory,
        &mut record,
        &mut child,
        &runtime_identity,
    )
    .await;
    drop(containment);
    join_log_tasks(stdout_task, stderr_task).await;
    Ok(())
}

async fn monitor_runtime(
    layout: &RuntimeLayout,
    directory: &Path,
    record: &mut SessionRecord,
    child: &mut tokio::process::Child,
    identity: &ProcessIdentity,
) {
    loop {
        if stop_requested(directory) {
            process::terminate_runtime_group(identity, false);
            wait_then_force(child, identity).await;
            record.state = RuntimeState::Stopped;
            record.failure_code = None;
            let _ = write_session(layout, record);
            return;
        }
        if child.try_wait().ok().flatten().is_some() {
            record.state = RuntimeState::Failed;
            record.failure_code = Some(BackendErrorCode::RuntimeExited);
            let _ = write_session(layout, record);
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_then_force(child: &mut tokio::process::Child, identity: &ProcessIdentity) {
    let deadline = tokio::time::Instant::now() + STOP_GRACE_PERIOD;
    while tokio::time::Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    process::terminate_runtime_group(identity, true);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}

fn runtime_command(
    deployment: &Path,
    override_path: &Path,
    temp_directory: &Path,
    payload: &SupervisorPayload,
) -> Result<tokio::process::Command, String> {
    ensure_direct_directory(deployment)?;
    ensure_direct_directory(temp_directory)?;
    let default_config = deployment.join("etc/Default");
    let mut command = if cfg!(windows) {
        let launcher = deployment.join("bin/start.ps1");
        ensure_direct_file(&launcher)?;
        let mut command = tokio::process::Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        command.arg(launcher);
        command
    } else {
        let launcher = deployment.join("bin/start");
        ensure_direct_file(&launcher)?;
        tokio::process::Command::new(launcher)
    };
    if default_config.is_file() {
        command.arg(default_config);
    }
    command.arg(override_path);
    command
        .current_dir(deployment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false)
        .env_clear()
        .env("JAVA_HOME", &payload.java_home)
        .env("M2EE_ADMIN_PASS", &payload.admin_password)
        .env("MX_LOG_LEVEL", "INFO")
        .env("TEMP", temp_directory)
        .env("TMP", temp_directory);
    let java_bin = Path::new(&payload.java_home).join("bin");
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let joined = std::env::join_paths(
        std::iter::once(java_bin).chain(std::env::split_paths(&inherited_path)),
    )
    .map_err(|error| format!("could not construct the runtime PATH: {error}"))?;
    command.env("PATH", joined);
    for name in [
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "SYSTEMDRIVE",
        "COMSPEC",
        "PATHEXT",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for (name, value) in &payload.environment {
        command.env(name, value);
    }
    process::configure_runtime_child(&mut command);
    Ok(command)
}

async fn capture_log<R>(
    stream: R,
    label: &'static str,
    sink: Arc<Mutex<LogSink>>,
    redactor: Arc<SecretRedactor>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        let count = match reader.read_until(b'\n', &mut line).await {
            Ok(count) => count,
            Err(_) => return,
        };
        if count == 0 {
            return;
        }
        if line.len() > 1024 * 1024 {
            line.truncate(1024 * 1024);
        }
        let text = String::from_utf8_lossy(&line);
        let text = redactor.redact(text.trim_end_matches(['\r', '\n']));
        let entry = format!("{} {label} {text}\n", Utc::now().to_rfc3339());
        let mut sink = sink.lock().await;
        if sink.saturated {
            continue;
        }
        if sink.written.saturating_add(entry.len() as u64) > MAX_LOG_BYTES {
            let _ = sink
                .file
                .write_all(b"mendimaru log limit reached; further output was discarded\n")
                .await;
            let _ = sink.file.flush().await;
            sink.saturated = true;
            continue;
        }
        if sink.file.write_all(entry.as_bytes()).await.is_err() {
            sink.saturated = true;
            continue;
        }
        sink.written += entry.len() as u64;
        let _ = sink.file.flush().await;
    }
}

async fn join_log_tasks(
    stdout: Option<tokio::task::JoinHandle<()>>,
    stderr: Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(task) = stdout {
        join_log_task(task).await;
    }
    if let Some(task) = stderr {
        join_log_task(task).await;
    }
}

async fn join_log_task(mut task: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(Duration::from_secs(2), &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

async fn append_internal_log(sink: &Arc<Mutex<LogSink>>, message: &str) {
    let entry = format!("{} supervisor {message}\n", Utc::now().to_rfc3339());
    let mut sink = sink.lock().await;
    if sink.saturated || sink.written.saturating_add(entry.len() as u64) > MAX_LOG_BYTES {
        return;
    }
    if sink.file.write_all(entry.as_bytes()).await.is_ok() {
        sink.written += entry.len() as u64;
        let _ = sink.file.flush().await;
    }
}

async fn fail_and_respond(
    layout: &RuntimeLayout,
    record: &mut SessionRecord,
    code: BackendErrorCode,
) {
    record.state = RuntimeState::Failed;
    record.failure_code = Some(code);
    let _ = write_session(layout, record);
    let _ = write_supervisor_response(&SupervisorResponse {
        ok: false,
        status: None,
        error: Some(runtime_error(
            record.backend,
            CapabilityId::RuntimeStart,
            code,
            Some(record.log_artifact.artifact_id.clone()),
            false,
        )),
    })
    .await;
}

async fn write_supervisor_response(response: &SupervisorResponse) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(response)
        .map_err(|error| format!("could not serialize the runtime handshake: {error}"))?;
    bytes.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(&bytes)
        .await
        .map_err(|error| format!("could not write the runtime handshake: {error}"))?;
    stdout
        .flush()
        .await
        .map_err(|error| format!("could not flush the runtime handshake: {error}"))
}

async fn http_ready(url: &str, admin_port: u16) -> bool {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    let admin_health = format!("http://127.0.0.1:{admin_port}/probes/ready");
    if client
        .get(&admin_health)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
    {
        return true;
    }
    let health = format!("{}/health/ready", url.trim_end_matches('/'));
    match client.get(&health).send().await {
        Ok(response) if response.status().is_success() => true,
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => client
            .get(url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success()),
        _ => false,
    }
}

fn runtime_environment() -> Result<BTreeMap<String, String>, String> {
    let Some(raw) = std::env::var_os(ENVIRONMENT_JSON) else {
        return Ok(BTreeMap::new());
    };
    std::env::remove_var(ENVIRONMENT_JSON);
    let mut raw = raw
        .into_string()
        .map_err(|_| "the runtime environment JSON must be UTF-8".to_string())?;
    if raw.len() > MAX_ENVIRONMENT_BYTES {
        raw.zeroize();
        return Err("the runtime environment JSON is too large".to_string());
    }
    let parsed = serde_json::from_str::<BTreeMap<String, String>>(&raw)
        .map_err(|_| "the runtime environment JSON is invalid".to_string());
    raw.zeroize();
    let environment = parsed?;
    validate_environment(&environment)?;
    Ok(environment)
}

fn validate_payload(payload: &SupervisorPayload) -> Result<(), String> {
    if payload.admin_password.len() < 32 || payload.admin_password.len() > 256 {
        return Err("the runtime admin credential is invalid".to_string());
    }
    validate_environment(&payload.environment)?;
    validate_private_absolute_path(&payload.java_home, true)?;
    validate_private_absolute_path(&payload.java_executable, false)?;
    Ok(())
}

fn validate_environment(environment: &BTreeMap<String, String>) -> Result<(), String> {
    if environment.len() > MAX_ENVIRONMENT_VALUES {
        return Err("too many runtime environment values were provided".to_string());
    }
    for (name, value) in environment {
        if !valid_environment_name(name)
            || value.len() < 3
            || value.len() > MAX_ENVIRONMENT_VALUE_BYTES
            || value.contains('\0')
            || reserved_environment_name(name)
        {
            return Err("a runtime environment value is invalid or reserved".to_string());
        }
    }
    Ok(())
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_uppercase())
        && name.len() <= 128
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn reserved_environment_name(name: &str) -> bool {
    name == "M2EE_ADMIN_PASS"
        || name == "JAVA_HOME"
        || name == "PATH"
        || name == ENVIRONMENT_JSON
        || name.starts_with("MENDIMARU_")
        || matches!(
            name,
            "ADMIN_PORT"
                | "ADMIN_ADDRESSES"
                | "ADMIN_ADMINPASSWORD"
                | "ADMIN_MONITORINGPASSWORD"
                | "RUNTIME_HTTP_PORT"
                | "RUNTIME_HTTP_ADDRESSES"
                | "RUNTIME_PARAMS_APPLICATIONROOTURL"
        )
}

fn copy_deployment(source: &Path, destination: &Path) -> Result<(), String> {
    ensure_direct_directory(source)?;
    if destination.exists() {
        return Err("the runtime deployment destination already exists".to_string());
    }
    store::ensure_private_directory(destination)?;
    let mut count = 0_u64;
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|error| format!("could not scan the deployment: {error}"))?;
        if entry.depth() == 0 {
            continue;
        }
        count += 1;
        if count > 250_000 {
            return Err("the runtime deployment contains too many files".to_string());
        }
        if entry.file_type().is_symlink() {
            return Err("the runtime deployment contains a symbolic link".to_string());
        }
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| "a runtime deployment file escaped its source".to_string())?;
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("the runtime deployment contains an unsafe path".to_string());
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            store::ensure_private_directory(&target)?;
        } else if entry.file_type().is_file() {
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("could not inspect the deployment: {error}"))?;
            total = total
                .checked_add(metadata.len())
                .filter(|value| *value <= 16 * 1024 * 1024 * 1024)
                .ok_or_else(|| "the runtime deployment exceeds the copy limit".to_string())?;
            if let Some(parent) = target.parent() {
                store::ensure_private_directory(parent)?;
            }
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("could not copy the runtime deployment: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let executable = metadata.permissions().mode() & 0o111 != 0;
                fs::set_permissions(
                    &target,
                    fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
                )
                .map_err(|error| format!("could not protect the deployment file: {error}"))?;
            }
        } else {
            return Err("the runtime deployment contains a special file".to_string());
        }
    }
    Ok(())
}

fn available_loopback_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("could not allocate a loopback port: {error}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| format!("could not inspect a loopback port: {error}"))
}

fn random_secret() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("could not generate a runtime admin credential: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn wait_for_identity(pid: u32) -> Result<ProcessIdentity, String> {
    for _ in 0..40 {
        if let Ok(identity) = process::identity(pid) {
            return Ok(identity);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("the process identity could not be established".to_string())
}

fn load_session(
    layout: &RuntimeLayout,
    session_id: &str,
    backend: BackendId,
    capability: CapabilityId,
) -> BackendResult<SessionRecord> {
    store::validate_identifier(session_id, "runtime").map_err(|_| {
        runtime_error(
            backend,
            capability,
            BackendErrorCode::InvalidRequest,
            None,
            false,
        )
    })?;
    let record: SessionRecord =
        store::read_json(&layout.session_record(session_id).map_err(|_| {
            runtime_error(
                backend,
                capability,
                BackendErrorCode::InvalidRequest,
                None,
                false,
            )
        })?)
        .map_err(|_| {
            runtime_error(
                backend,
                capability,
                BackendErrorCode::RuntimeSessionNotFound,
                None,
                false,
            )
        })?;
    if record.schema_version != CONTRACT_SCHEMA_VERSION
        || record.session_id != session_id
        || record.backend != backend
    {
        return Err(runtime_error(
            backend,
            capability,
            BackendErrorCode::RuntimeSessionNotFound,
            None,
            false,
        ));
    }
    Ok(record)
}

fn write_session(layout: &RuntimeLayout, record: &SessionRecord) -> Result<(), String> {
    store::write_json(&layout.session_record(&record.session_id)?, record)
}

fn status_from_record(record: &SessionRecord) -> RuntimeStatus {
    RuntimeStatus {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        session_id: record.session_id.clone(),
        mode: RuntimeMode::Portable,
        state: record.state,
        process_id: record.runtime_pid,
        started_at: record.started_at,
        url: (record.state == RuntimeState::Ready).then(|| record.url.clone()),
        failure_code: record.failure_code,
        log_artifact: Some(record.log_artifact.clone()),
    }
}

fn supervisor_identity(record: &SessionRecord) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        pid: record.supervisor_pid?,
        start_token: record.supervisor_start_token.clone()?,
    })
}

fn runtime_identity(record: &SessionRecord) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        pid: record.runtime_pid?,
        start_token: record.runtime_start_token.clone()?,
    })
}

fn session_supervisor_alive(record: &SessionRecord) -> bool {
    supervisor_identity(record).is_some_and(|identity| process::matches(&identity))
}

fn stop_requested(directory: &Path) -> bool {
    fs::symlink_metadata(directory.join("stop.request"))
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn validate_private_absolute_path(value: &str, directory: bool) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err("a runtime dependency path is not absolute".to_string());
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("could not resolve a runtime dependency: {error}"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("could not inspect a runtime dependency: {error}"))?;
    if directory != metadata.is_dir() || (!directory && !metadata.is_file()) {
        return Err("a runtime dependency has the wrong type".to_string());
    }
    Ok(canonical)
}

fn ensure_direct_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect a runtime directory: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("a runtime directory is not direct".to_string());
    }
    Ok(())
}

fn ensure_direct_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect a runtime file: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("a runtime file is not direct".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_redaction_covers_raw_and_encoded_values() {
        let mut environment = BTreeMap::new();
        environment.insert("LICENSE_TOKEN".to_string(), "sensitive-value".to_string());
        let payload = SupervisorPayload {
            admin_password: "admin-secret-value-that-is-long-enough".to_string(),
            environment,
            java_home: "/java".to_string(),
            java_executable: "/java/bin/java".to_string(),
        };
        let redactor = SecretRedactor::new(
            &payload,
            &["/private/runtime/session/deployment".to_string()],
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode("sensitive-value");
        let output = redactor.redact(&format!(
            "raw=sensitive-value encoded={encoded} admin=admin-secret-value-that-is-long-enough"
        ));
        assert!(!output.contains("sensitive-value"));
        assert!(!output.contains(&encoded));
        assert!(!output.contains("admin-secret"));
        assert_eq!(output.matches("[REDACTED]").count(), 3);
        assert_eq!(
            redactor.redact("path=/private/runtime/session/deployment/app"),
            "path=[REDACTED]/app"
        );
    }

    #[test]
    fn environment_contract_rejects_reserved_or_ambiguous_names() {
        let valid = BTreeMap::from([("DATABASE_PASSWORD".to_string(), "secret".to_string())]);
        assert!(validate_environment(&valid).is_ok());
        for name in [
            "M2EE_ADMIN_PASS",
            "JAVA_HOME",
            "PATH",
            "MENDIMARU_CACHE_DIR",
            "ADMIN_PORT",
            "ADMIN_ADDRESSES",
            "ADMIN_ADMINPASSWORD",
            "RUNTIME_HTTP_PORT",
            "RUNTIME_HTTP_ADDRESSES",
            "RUNTIME_PARAMS_APPLICATIONROOTURL",
            "lowercase",
            "A-B",
        ] {
            let value = BTreeMap::from([(name.to_string(), "secret".to_string())]);
            assert!(validate_environment(&value).is_err(), "accepted {name}");
        }
        assert!(validate_environment(&BTreeMap::from([(
            "SHORT_VALUE".to_string(),
            "xy".to_string(),
        )]))
        .is_err());
    }
}
