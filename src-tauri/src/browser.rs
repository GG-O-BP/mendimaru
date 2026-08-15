use crate::app_paths::AppPaths;
use crate::contracts::{
    ArtifactDescriptor, ArtifactKind, BackendError, BackendErrorCode, BackendId, BackendResult,
    BrowserTestCaseSummary, BrowserTestOutcome, BrowserTestPolicy, BrowserTestRequest,
    BrowserTestSummary, CapabilityId, PlatformId, RuntimeMode, CONTRACT_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use zip::ZipArchive;

const STORE_DIRECTORY: &str = "browser-tests";
const MAX_SUITE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RUNNER_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARTIFACT_FILES: usize = 512;
const MAX_STORE_BYTES: u64 = 1024 * 1024 * 1024;
const RUNNER_PATH_OVERRIDE: &str = "MENDIMARU_BROWSER_RUNNER_PATH";
const NODE_BINARY_OVERRIDE: &str = "MENDIMARU_NODE_BINARY";
const RUNNER_VERSION: &str = "1.0.0";
const MINIMUM_NODE_VERSION: &str = "22.22.2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserDoctor {
    pub schema_version: String,
    pub runner_version: String,
    pub ready: bool,
    pub node_version: String,
    pub minimum_node_version: String,
    pub node_supported: bool,
    pub playwright_version: String,
    pub chromium: ChromiumDiagnostic,
    pub download_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChromiumDiagnostic {
    pub installed: bool,
    pub launchable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerEnvelope {
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<RunnerError>,
}

#[derive(Debug, Deserialize)]
struct RunnerError {
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerSummary {
    schema_version: String,
    session_id: String,
    outcome: BrowserTestOutcome,
    passed: u32,
    failed: u32,
    skipped: u32,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    browser_name: String,
    browser_version: String,
    playwright_version: String,
    tests: Vec<BrowserTestCaseSummary>,
    files: Vec<RunnerArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunnerArtifact {
    path: String,
    kind: ArtifactKind,
    media_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunnerManifest {
    schema_version: String,
    session_id: String,
    created_at: DateTime<Utc>,
    host_platform: PlatformId,
    studio_platform: PlatformId,
    #[serde(default)]
    runtime_platform: Option<PlatformId>,
    backend: BackendId,
    runtime_mode: RuntimeMode,
    #[serde(default)]
    studio_version: Option<String>,
    #[serde(default)]
    runtime_version: Option<String>,
    browser: RunnerBrowserIdentity,
    playwright_version: String,
    runner_version: String,
    suite: RunnerSuiteIdentity,
    policy: BrowserTestPolicy,
    artifacts: Vec<RunnerManifestArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerBrowserIdentity {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerSuiteIdentity {
    name: String,
    tests: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunnerManifestArtifact {
    file: String,
    kind: ArtifactKind,
    media_type: String,
    sha256: String,
    size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunnerRequest<'a> {
    schema_version: &'static str,
    session_id: &'a str,
    base_url: &'a str,
    output_directory: &'a Path,
    runtime_context: &'a crate::contracts::BrowserRuntimeContext,
    policy: &'a crate::contracts::BrowserTestPolicy,
    suite: &'a Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactRecord {
    descriptor: ArtifactDescriptor,
    relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserRunRecord {
    schema_version: String,
    backend: BackendId,
    summary: BrowserTestSummary,
    artifacts: Vec<ArtifactRecord>,
}

#[derive(Debug)]
struct BrowserStore {
    runs: PathBuf,
    lock: File,
}

struct StagingGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

impl BrowserStore {
    fn discover() -> Result<Self, String> {
        let paths = AppPaths::discover_for_cli()?;
        paths.ensure_cache_directory()?;
        let root = direct_child(paths.cache_directory(), STORE_DIRECTORY)?;
        ensure_private_directory(&root)?;
        let lock = open_store_lock(&direct_child(&root, "store.lock")?)?;
        let runs = direct_child(&root, "runs")?;
        ensure_private_directory(&runs)?;
        Ok(Self { runs, lock })
    }

    fn staging_directory(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_session_id(session_id)?;
        let suffix = random_suffix()?;
        let path = self.runs.join(format!(".{session_id}.{suffix}.staging"));
        fs::create_dir(&path)
            .map_err(|error| format!("could not create browser staging: {error}"))?;
        set_directory_permissions(&path)?;
        Ok(path)
    }

    fn run_directory(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_session_id(session_id)?;
        Ok(self.runs.join(session_id))
    }

    fn commit(&self, staging: &Path, session_id: &str) -> Result<PathBuf, String> {
        ensure_direct_directory(staging, "browser staging")?;
        let destination = self.run_directory(session_id)?;
        if fs::symlink_metadata(&destination).is_ok() {
            return Err("the browser session already exists".to_string());
        }
        fs::rename(staging, &destination)
            .map_err(|error| format!("could not commit browser artifacts: {error}"))?;
        Ok(destination)
    }

    fn commit_and_prune(
        &self,
        staging: &Path,
        session_id: &str,
        retention_runs: u32,
    ) -> Result<PathBuf, String> {
        self.lock_exclusive()?;
        let destination = self.commit(staging, session_id)?;
        // Cleanup is best effort after the durable commit. A retention failure must not
        // turn a completed run into a retryable error for an already-used session ID.
        let _ = self.prune(session_id, retention_runs);
        Ok(destination)
    }

    fn lock_exclusive(&self) -> Result<(), String> {
        fs2::FileExt::lock_exclusive(&self.lock)
            .map_err(|error| format!("could not lock the browser artifact store: {error}"))
    }

    fn prune(&self, current: &str, retention_runs: u32) -> Result<(), String> {
        let mut runs = Vec::new();
        for entry in fs::read_dir(&self.runs)
            .map_err(|error| format!("could not list browser runs: {error}"))?
        {
            let entry = entry.map_err(|error| format!("could not inspect browser run: {error}"))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("could not inspect browser run: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            if validate_session_id(&name).is_err() {
                continue;
            }
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            let bytes = directory_size(&entry.path())?;
            runs.push((name, entry.path(), modified, bytes));
        }
        runs.sort_by_key(|(_, _, modified, _)| std::cmp::Reverse(*modified));
        if let Some(position) = runs.iter().position(|(name, _, _, _)| name == current) {
            let current_run = runs.remove(position);
            runs.insert(0, current_run);
        }
        let mut kept = 0_u32;
        let mut total = 0_u64;
        for (name, path, _, bytes) in runs {
            let retain = name == current
                || (kept < retention_runs && total.saturating_add(bytes) <= MAX_STORE_BYTES);
            if retain {
                kept = kept.saturating_add(1);
                total = total.saturating_add(bytes);
            } else {
                fs::remove_dir_all(&path)
                    .map_err(|error| format!("could not prune browser artifacts: {error}"))?;
            }
        }
        Ok(())
    }
}

pub(crate) async fn doctor(backend: BackendId) -> BackendResult<BrowserDoctor> {
    let value = invoke_runner("doctor", None, backend, CapabilityId::BrowserTest).await?;
    parse_doctor(value, backend)
}

pub(crate) async fn install_chromium(backend: BackendId) -> BackendResult<BrowserDoctor> {
    let value = invoke_runner("install", None, backend, CapabilityId::BrowserTest).await?;
    parse_doctor(value, backend)
}

fn parse_doctor(value: Value, backend: BackendId) -> BackendResult<BrowserDoctor> {
    let doctor: BrowserDoctor = serde_json::from_value(value).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    let launch_identity_valid = !doctor.chromium.launchable
        || (doctor.chromium.installed
            && doctor
                .chromium
                .version
                .as_ref()
                .is_some_and(|version| !version.is_empty() && version.len() <= 80));
    let node_supported =
        numeric_version(&doctor.node_version).is_some_and(|version| version >= [22, 22, 2]);
    if doctor.schema_version != CONTRACT_SCHEMA_VERSION
        || doctor.runner_version != RUNNER_VERSION
        || doctor.minimum_node_version != MINIMUM_NODE_VERSION
        || doctor.node_version.is_empty()
        || doctor.node_version.len() > 80
        || doctor.playwright_version.is_empty()
        || doctor.playwright_version.len() > 80
        || doctor.download_policy != "explicit-only"
        || doctor.node_supported != node_supported
        || doctor.ready
            != (doctor.node_supported && doctor.chromium.installed && doctor.chromium.launchable)
        || !launch_identity_valid
    {
        return Err(browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        ));
    }
    Ok(doctor)
}

fn numeric_version(value: &str) -> Option<[u64; 3]> {
    let mut parts = value.split('.');
    let version = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(version)
}

pub(crate) async fn test(
    request: &BrowserTestRequest,
    backend: BackendId,
) -> BackendResult<BrowserTestSummary> {
    validate_session_id(&request.session_id).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::InvalidRequest,
            false,
        )
    })?;
    validate_policy(request).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::InvalidRequest,
            false,
        )
    })?;
    if request.runtime_context.backend != backend {
        return Err(browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::InvalidRequest,
            false,
        ));
    }
    let suite = read_suite(Path::new(&request.suite_path)).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::InvalidRequest,
            false,
        )
    })?;
    let secrets = secret_values(&suite).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::PreconditionFailed,
            false,
        )
    })?;
    let store = BrowserStore::discover().map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::PreconditionFailed,
            true,
        )
    })?;
    let staging = store.staging_directory(&request.session_id).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            true,
        )
    })?;
    let mut guard = StagingGuard {
        path: staging.clone(),
        armed: true,
    };
    let runner_request = serde_json::to_value(RunnerRequest {
        schema_version: CONTRACT_SCHEMA_VERSION,
        session_id: &request.session_id,
        base_url: &request.base_url,
        output_directory: &staging,
        runtime_context: &request.runtime_context,
        policy: &request.policy,
        suite: &suite,
    })
    .map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    let value = invoke_runner(
        "run",
        Some(&runner_request),
        backend,
        CapabilityId::BrowserTest,
    )
    .await?;
    let runner: RunnerSummary = serde_json::from_value(value).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    validate_runner_summary(&runner, request, &suite).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    let artifacts = materialize_artifacts(
        &staging,
        &runner.files,
        &request.session_id,
        backend,
        request.policy.max_artifact_bytes,
    )
    .map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    verify_runner_manifest(&staging, &runner.files, request, &runner, &suite).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    verify_secret_free(&staging, &secrets).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    let summary = BrowserTestSummary {
        schema_version: runner.schema_version,
        session_id: runner.session_id,
        outcome: runner.outcome,
        passed: runner.passed,
        failed: runner.failed,
        skipped: runner.skipped,
        started_at: runner.started_at,
        finished_at: runner.finished_at,
        browser_name: runner.browser_name,
        browser_version: runner.browser_version,
        playwright_version: runner.playwright_version,
        tests: runner.tests,
        artifacts: artifacts
            .iter()
            .map(|record| record.descriptor.clone())
            .collect(),
    };
    let record = BrowserRunRecord {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        backend,
        summary: summary.clone(),
        artifacts,
    };
    write_json_private(&staging.join("index.json"), &record).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            true,
        )
    })?;
    store
        .commit_and_prune(&staging, &request.session_id, request.policy.retention_runs)
        .map_err(|_| {
            browser_error(
                backend,
                CapabilityId::BrowserTest,
                BackendErrorCode::OperationFailed,
                true,
            )
        })?;
    guard.armed = false;
    Ok(summary)
}

pub(crate) fn artifacts(
    session_id: &str,
    backend: BackendId,
) -> BackendResult<Vec<ArtifactDescriptor>> {
    validate_session_id(session_id).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserArtifacts,
            BackendErrorCode::InvalidRequest,
            false,
        )
    })?;
    let store = BrowserStore::discover().map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserArtifacts,
            BackendErrorCode::PreconditionFailed,
            true,
        )
    })?;
    store.lock_exclusive().map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserArtifacts,
            BackendErrorCode::OperationFailed,
            true,
        )
    })?;
    let run = store.run_directory(session_id).map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserArtifacts,
            BackendErrorCode::InvalidRequest,
            false,
        )
    })?;
    ensure_direct_directory(&run, "browser run").map_err(|_| {
        browser_error(
            backend,
            CapabilityId::BrowserArtifacts,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    let record: BrowserRunRecord = read_json_bounded(&run.join("index.json"), MAX_INDEX_BYTES)
        .map_err(|_| {
            browser_error(
                backend,
                CapabilityId::BrowserArtifacts,
                BackendErrorCode::OperationFailed,
                false,
            )
        })?;
    let indexed_descriptors = record
        .artifacts
        .iter()
        .map(|artifact| artifact.descriptor.clone())
        .collect::<Vec<_>>();
    if record.schema_version != CONTRACT_SCHEMA_VERSION
        || record.backend != backend
        || record.summary.schema_version != CONTRACT_SCHEMA_VERSION
        || record.summary.session_id != session_id
        || record.summary.artifacts != indexed_descriptors
        || record.artifacts.iter().any(|artifact| {
            artifact.descriptor.schema_version != CONTRACT_SCHEMA_VERSION
                || artifact.descriptor.session_id != session_id
                || artifact.descriptor.backend != backend
        })
    {
        return Err(browser_error(
            backend,
            CapabilityId::BrowserArtifacts,
            BackendErrorCode::OperationFailed,
            false,
        ));
    }
    for artifact in &record.artifacts {
        verify_artifact(&run, artifact).map_err(|_| {
            browser_error(
                backend,
                CapabilityId::BrowserArtifacts,
                BackendErrorCode::OperationFailed,
                false,
            )
        })?;
    }
    Ok(record
        .artifacts
        .into_iter()
        .map(|record| record.descriptor)
        .collect())
}

async fn invoke_runner(
    command_name: &str,
    request: Option<&Value>,
    backend: BackendId,
    capability: CapabilityId,
) -> BackendResult<Value> {
    let runner = runner_path().map_err(|_| {
        browser_error(
            backend,
            capability,
            BackendErrorCode::PreconditionFailed,
            false,
        )
    })?;
    let node = node_binary().map_err(|_| {
        browser_error(
            backend,
            capability,
            BackendErrorCode::PreconditionFailed,
            false,
        )
    })?;
    let mut command = tokio::process::Command::new(node);
    command
        .arg(runner)
        .arg(command_name)
        .stdin(if request.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|_| {
        browser_error(
            backend,
            capability,
            BackendErrorCode::PreconditionFailed,
            false,
        )
    })?;
    if let Some(request) = request {
        let bytes = serde_json::to_vec(request).map_err(|_| {
            browser_error(
                backend,
                capability,
                BackendErrorCode::OperationFailed,
                false,
            )
        })?;
        let mut stdin = child.stdin.take().ok_or_else(|| {
            browser_error(
                backend,
                capability,
                BackendErrorCode::OperationFailed,
                false,
            )
        })?;
        stdin.write_all(&bytes).await.map_err(|_| {
            browser_error(backend, capability, BackendErrorCode::OperationFailed, true)
        })?;
        stdin.shutdown().await.map_err(|_| {
            browser_error(backend, capability, BackendErrorCode::OperationFailed, true)
        })?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|_| browser_error(backend, capability, BackendErrorCode::OperationFailed, true))?;
    if output.stdout.len() > MAX_RUNNER_OUTPUT_BYTES
        || output.stdout.iter().filter(|b| **b == b'\n').count() != 1
    {
        return Err(browser_error(
            backend,
            capability,
            BackendErrorCode::OperationFailed,
            false,
        ));
    }
    let envelope: RunnerEnvelope = serde_json::from_slice(&output.stdout).map_err(|_| {
        browser_error(
            backend,
            capability,
            BackendErrorCode::OperationFailed,
            false,
        )
    })?;
    if !envelope.ok || !output.status.success() {
        let code = envelope.error.as_ref().map(|error| error.code.as_str());
        let (error_code, retryable) = match code {
            Some("invalid_request" | "invalid_suite" | "contract_mismatch") => {
                (BackendErrorCode::InvalidRequest, false)
            }
            Some("chromium_unavailable" | "auth_value_missing" | "node_unsupported") => {
                (BackendErrorCode::PreconditionFailed, false)
            }
            Some("chromium_install_failed") => (BackendErrorCode::OperationFailed, true),
            _ => (BackendErrorCode::OperationFailed, false),
        };
        return Err(browser_error(backend, capability, error_code, retryable));
    }
    envelope.data.ok_or_else(|| {
        browser_error(
            backend,
            capability,
            BackendErrorCode::OperationFailed,
            false,
        )
    })
}

fn runner_path() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os(RUNNER_PATH_OVERRIDE) {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err("the browser runner override must be absolute".to_string());
        }
        validate_runner_file(&path)?;
        return Ok(path);
    }
    if cfg!(debug_assertions) {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| "the browser runner directory is unavailable".to_string())?
            .join("scripts/browser-runner.mjs");
        if validate_runner_file(&source).is_ok() {
            return Ok(source);
        }
    }
    let package = tauri::PackageInfo {
        name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION")
            .parse()
            .map_err(|_| "the application package version is invalid".to_string())?,
        authors: env!("CARGO_PKG_AUTHORS"),
        description: env!("CARGO_PKG_DESCRIPTION"),
        crate_name: env!("CARGO_PKG_NAME"),
    };
    let resource = tauri::utils::platform::resource_dir(&package, &tauri::Env::default())
        .map_err(|error| format!("could not resolve application resources: {error}"))?
        .join("browser/browser-runner.mjs");
    validate_runner_file(&resource)?;
    Ok(resource)
}

fn validate_runner_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect browser runner: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("the browser runner must be a direct regular file".to_string());
    }
    Ok(())
}

fn node_binary() -> Result<PathBuf, String> {
    let Some(value) = std::env::var_os(NODE_BINARY_OVERRIDE) else {
        return Ok(PathBuf::from("node"));
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("the Node.js override must be absolute".to_string());
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("could not inspect Node.js: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("the Node.js override must be a direct regular file".to_string());
    }
    Ok(path)
}

fn validate_policy(request: &BrowserTestRequest) -> Result<(), String> {
    for timeout in [
        request.policy.navigation_timeout_milliseconds,
        request.policy.action_timeout_milliseconds,
        request.policy.assertion_timeout_milliseconds,
    ] {
        if !(100..=300_000).contains(&timeout) {
            return Err("browser timeout is outside the supported range".to_string());
        }
    }
    if !(1_048_576..=536_870_912).contains(&request.policy.max_artifact_bytes)
        || !(1..=100).contains(&request.policy.retention_runs)
    {
        return Err("browser artifact policy is outside the supported range".to_string());
    }
    Ok(())
}

fn read_suite(path: &Path) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect browser suite: {error}"))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_SUITE_BYTES
        || path.extension().and_then(|value| value.to_str()) != Some("json")
    {
        return Err("the browser suite must be a bounded direct JSON file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(MAX_SUITE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("could not read browser suite: {error}"))?;
    if bytes.len() as u64 > MAX_SUITE_BYTES {
        return Err("the browser suite exceeds the size limit".to_string());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse browser suite: {error}"))
}

fn validate_runner_summary(
    summary: &RunnerSummary,
    request: &BrowserTestRequest,
    suite: &Value,
) -> Result<(), String> {
    let expected_tests = suite
        .get("tests")
        .and_then(Value::as_array)
        .ok_or_else(|| "the browser suite test inventory is invalid".to_string())?;
    let before_each = suite
        .get("beforeEach")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if summary.schema_version != CONTRACT_SCHEMA_VERSION
        || summary.session_id != request.session_id
        || summary.browser_name != "chromium"
        || summary.browser_version.is_empty()
        || summary.browser_version.len() > 80
        || summary.playwright_version.is_empty()
        || summary.playwright_version.len() > 80
        || summary.finished_at < summary.started_at
        || summary.files.is_empty()
        || summary.files.len() > MAX_ARTIFACT_FILES
        || summary.tests.is_empty()
        || summary.tests.len() > 100
        || summary.tests.len() != expected_tests.len()
    {
        return Err("the browser runner returned an invalid summary".to_string());
    }
    for (actual, expected) in summary.tests.iter().zip(expected_tests) {
        let expected_name = expected.get("name").and_then(Value::as_str).unwrap_or("");
        let test_steps = expected
            .get("steps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let expected_total = before_each.saturating_add(test_steps);
        let valid_failure = match actual.outcome {
            BrowserTestOutcome::Passed => {
                actual.failure.is_none() && actual.completed_steps == actual.total_steps
            }
            BrowserTestOutcome::Failed => actual
                .failure
                .as_ref()
                .is_some_and(|failure| !failure.is_empty() && failure.len() <= 8_192),
            BrowserTestOutcome::Skipped => actual.failure.is_none(),
        };
        if actual.name != expected_name
            || actual.name.is_empty()
            || actual.name.len() > 160
            || actual.total_steps as usize != expected_total
            || actual.completed_steps > actual.total_steps
            || !valid_failure
        {
            return Err("the browser test summary is inconsistent with its suite".to_string());
        }
    }
    let passed = summary
        .tests
        .iter()
        .filter(|test| test.outcome == BrowserTestOutcome::Passed)
        .count() as u32;
    let failed = summary
        .tests
        .iter()
        .filter(|test| test.outcome == BrowserTestOutcome::Failed)
        .count() as u32;
    let skipped = summary
        .tests
        .iter()
        .filter(|test| test.outcome == BrowserTestOutcome::Skipped)
        .count() as u32;
    if (summary.passed, summary.failed, summary.skipped) != (passed, failed, skipped)
        || (summary.failed == 0) != (summary.outcome == BrowserTestOutcome::Passed)
    {
        return Err("browser result counts are inconsistent".to_string());
    }
    Ok(())
}

fn materialize_artifacts(
    directory: &Path,
    files: &[RunnerArtifact],
    session_id: &str,
    backend: BackendId,
    maximum_bytes: u64,
) -> Result<Vec<ArtifactRecord>, String> {
    ensure_direct_directory(directory, "browser staging")?;
    let expected = files
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect::<BTreeSet<_>>();
    if expected.len() != files.len() {
        return Err("browser artifact paths are duplicated".to_string());
    }
    let actual = fs::read_dir(directory)
        .map_err(|error| format!("could not list browser artifacts: {error}"))?
        .map(|entry| {
            let entry = entry.map_err(|error| format!("could not inspect artifact: {error}"))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("could not inspect artifact: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("browser artifacts must be flat direct files".to_string());
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| "browser artifact name is not UTF-8".to_string())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual != expected.into_iter().map(str::to_string).collect() {
        return Err("browser artifact inventory does not match".to_string());
    }
    let mut records = Vec::with_capacity(files.len());
    let mut total = 0_u64;
    for artifact in files {
        validate_artifact_name(&artifact.path)?;
        if artifact.media_type.is_empty() || artifact.media_type.len() > 160 {
            return Err("browser artifact media type is invalid".to_string());
        }
        let path = directory.join(&artifact.path);
        let (sha256, size) = digest_file(&path)?;
        total = total.saturating_add(size);
        let mut descriptor = ArtifactDescriptor::create(session_id, backend, artifact.kind)
            .map_err(|error| error.to_string())?;
        descriptor.media_type = Some(artifact.media_type.clone());
        descriptor.location = Some(format!("mendimaru-cache://{}", descriptor.artifact_id));
        descriptor.sha256 = Some(sha256);
        descriptor.size_bytes = Some(size);
        records.push(ArtifactRecord {
            descriptor,
            relative_path: artifact.path.clone(),
        });
    }
    if total > maximum_bytes {
        return Err("browser artifact inventory exceeds the requested limit".to_string());
    }
    Ok(records)
}

fn verify_runner_manifest(
    directory: &Path,
    files: &[RunnerArtifact],
    request: &BrowserTestRequest,
    summary: &RunnerSummary,
    suite: &Value,
) -> Result<(), String> {
    let manifest_file = files
        .iter()
        .find(|artifact| artifact.path == "artifact-manifest.json")
        .ok_or_else(|| "the browser artifact manifest is missing".to_string())?;
    if manifest_file.kind != ArtifactKind::BrowserReport
        || manifest_file.media_type != "application/json"
    {
        return Err("the browser artifact manifest descriptor is invalid".to_string());
    }
    let manifest: RunnerManifest =
        read_json_bounded(&directory.join("artifact-manifest.json"), MAX_INDEX_BYTES)?;
    let context = &request.runtime_context;
    let suite_name = suite.get("name").and_then(Value::as_str).unwrap_or("");
    let suite_tests = suite
        .get("tests")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if manifest.schema_version != CONTRACT_SCHEMA_VERSION
        || manifest.session_id != request.session_id
        || manifest.created_at != summary.finished_at
        || manifest.host_platform != context.host_platform
        || manifest.studio_platform != context.studio_platform
        || manifest.runtime_platform != context.runtime_platform
        || manifest.backend != context.backend
        || manifest.runtime_mode != context.runtime_mode
        || manifest.studio_version != context.studio_version
        || manifest.runtime_version != context.runtime_version
        || manifest.browser.name != summary.browser_name
        || manifest.browser.version != summary.browser_version
        || manifest.playwright_version != summary.playwright_version
        || manifest.runner_version != RUNNER_VERSION
        || manifest.suite.name != suite_name
        || manifest.suite.tests != suite_tests
        || manifest.policy != request.policy
    {
        return Err("the browser artifact manifest metadata is inconsistent".to_string());
    }

    let expected = files
        .iter()
        .filter(|artifact| artifact.path != "artifact-manifest.json")
        .map(|artifact| (artifact.path.as_str(), artifact))
        .collect::<BTreeMap<_, _>>();
    let reported = manifest
        .artifacts
        .iter()
        .map(|artifact| (artifact.file.as_str(), artifact))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != files.len().saturating_sub(1)
        || reported.len() != manifest.artifacts.len()
        || expected.keys().ne(reported.keys())
    {
        return Err("the browser artifact manifest inventory is inconsistent".to_string());
    }
    for (name, expected_artifact) in expected {
        let reported_artifact = reported
            .get(name)
            .ok_or_else(|| "the browser artifact manifest entry is missing".to_string())?;
        let (sha256, size_bytes) = digest_file(&directory.join(name))?;
        if reported_artifact.kind != expected_artifact.kind
            || reported_artifact.media_type != expected_artifact.media_type
            || reported_artifact.sha256 != sha256
            || reported_artifact.size_bytes != size_bytes
        {
            return Err("the browser artifact manifest digest is inconsistent".to_string());
        }
    }
    Ok(())
}

fn verify_artifact(directory: &Path, record: &ArtifactRecord) -> Result<(), String> {
    validate_artifact_name(&record.relative_path)?;
    if record.descriptor.location.as_deref()
        != Some(&format!(
            "mendimaru-cache://{}",
            record.descriptor.artifact_id
        ))
    {
        return Err("browser artifact location is invalid".to_string());
    }
    let (sha256, size) = digest_file(&directory.join(&record.relative_path))?;
    if record.descriptor.sha256.as_deref() != Some(&sha256)
        || record.descriptor.size_bytes != Some(size)
    {
        return Err("browser artifact integrity check failed".to_string());
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<(String, u64), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect browser artifact: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("browser artifact must be a direct regular file".to_string());
    }
    let mut file =
        File::open(path).map_err(|error| format!("could not open browser artifact: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not read browser artifact: {error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((format!("{:x}", digest.finalize()), metadata.len()))
}

fn secret_values(suite: &Value) -> Result<Vec<Vec<u8>>, String> {
    let mut names = BTreeSet::new();
    collect_environment_names(suite, &mut names);
    let mut values = BTreeSet::new();
    for name in names {
        let value =
            std::env::var(&name).map_err(|_| "a browser test secret is unavailable".to_string())?;
        if !value.is_empty() {
            values.insert(value);
        }
    }
    if let Some(storage_name) = suite.get("storageStateEnv").and_then(Value::as_str) {
        let path = std::env::var_os(storage_name)
            .map(PathBuf::from)
            .ok_or_else(|| "the browser storage state is unavailable".to_string())?;
        if !path.is_absolute() {
            return Err("the browser storage state path must be absolute".to_string());
        }
        let state: Value = read_json_bounded(&path, 2 * 1024 * 1024)?;
        collect_storage_secrets(&state, &mut values);
    }
    let mut variants = BTreeSet::new();
    for value in values {
        variants.insert(value.as_bytes().to_vec());
        let percent_encoded = percent_encode(value.as_bytes(), true);
        variants.insert(percent_encoded.as_bytes().to_vec());
        variants.insert(percent_encoded.replace("%20", "+").into_bytes());
        let lowercase_percent = percent_encode(value.as_bytes(), false);
        variants.insert(lowercase_percent.as_bytes().to_vec());
        variants.insert(lowercase_percent.replace("%20", "+").into_bytes());
        let json = serde_json::to_string(&value)
            .map_err(|_| "could not encode a browser test secret".to_string())?;
        variants.insert(json.as_bytes()[1..json.len() - 1].to_vec());
        variants.insert(html_escape(&value, false).into_bytes());
        variants.insert(html_escape(&value, true).into_bytes());
        use base64::Engine;
        variants.insert(
            base64::engine::general_purpose::STANDARD
                .encode(&value)
                .into_bytes(),
        );
        variants.insert(
            base64::engine::general_purpose::URL_SAFE
                .encode(&value)
                .into_bytes(),
        );
        variants.insert(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(&value)
                .into_bytes(),
        );
    }
    Ok(variants
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect())
}

fn collect_environment_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_environment_names(value, names);
            }
        }
        Value::Object(values) => {
            if let Some(secret_names) = values.get("secretEnv").and_then(Value::as_array) {
                for name in secret_names.iter().filter_map(Value::as_str) {
                    if valid_test_environment_name(name) {
                        names.insert(name.to_string());
                    }
                }
            }
            let sensitive = values
                .get("sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if sensitive {
                if let Some(name) = values.get("valueFromEnv").and_then(Value::as_str) {
                    if valid_test_environment_name(name) {
                        names.insert(name.to_string());
                    }
                }
            }
            for value in values.values() {
                collect_environment_names(value, names);
            }
        }
        _ => {}
    }
}

fn collect_storage_secrets(value: &Value, values: &mut BTreeSet<String>) {
    if let Some(cookies) = value.get("cookies").and_then(Value::as_array) {
        for cookie in cookies {
            if let Some(secret) = cookie.get("value").and_then(Value::as_str) {
                if !secret.is_empty() {
                    values.insert(secret.to_string());
                }
            }
        }
    }
    if let Some(origins) = value.get("origins").and_then(Value::as_array) {
        for item in origins {
            if let Some(storage) = item.get("localStorage").and_then(Value::as_array) {
                for entry in storage {
                    if let Some(secret) = entry.get("value").and_then(Value::as_str) {
                        if !secret.is_empty() {
                            values.insert(secret.to_string());
                        }
                    }
                }
            }
        }
    }
}

fn verify_secret_free(directory: &Path, secrets: &[Vec<u8>]) -> Result<(), String> {
    if secrets.is_empty() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("could not scan browser artifacts: {error}"))?
    {
        let entry = entry.map_err(|error| format!("could not inspect artifact: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("could not inspect artifact: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("unsafe browser artifact encountered".to_string());
        }
        if entry.path().extension().and_then(|value| value.to_str()) == Some("zip") {
            let file = File::open(entry.path())
                .map_err(|error| format!("could not inspect trace archive: {error}"))?;
            let mut archive = ZipArchive::new(file)
                .map_err(|error| format!("could not parse trace archive: {error}"))?;
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|error| format!("could not inspect trace entry: {error}"))?;
                if secrets
                    .iter()
                    .any(|secret| contains_bytes(entry.name_raw(), secret))
                {
                    return Err("a trace entry name contains a private value".to_string());
                }
                let mut bytes = Vec::new();
                entry
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("could not read trace entry: {error}"))?;
                if secrets.iter().any(|secret| contains_bytes(&bytes, secret)) {
                    return Err("a browser artifact contains a private value".to_string());
                }
            }
        } else {
            let bytes = fs::read(entry.path())
                .map_err(|error| format!("could not scan browser artifact: {error}"))?;
            if secrets.iter().any(|secret| contains_bytes(&bytes, secret)) {
                return Err("a browser artifact contains a private value".to_string());
            }
        }
    }
    Ok(())
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn percent_encode(value: &[u8], uppercase: bool) -> String {
    value
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (*byte as char).to_string()
            } else {
                if uppercase {
                    format!("%{byte:02X}")
                } else {
                    format!("%{byte:02x}")
                }
            }
        })
        .collect()
}

fn html_escape(value: &str, quote: bool) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' if quote => escaped.push_str("&quot;"),
            '\'' if quote => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn valid_test_environment_name(value: &str) -> bool {
    value.strip_prefix("MENDIMARU_TEST_").is_some_and(|suffix| {
        (1..=64).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

fn browser_error(
    backend: BackendId,
    capability: CapabilityId,
    code: BackendErrorCode,
    retryable: bool,
) -> BackendError {
    BackendError {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        code,
        message: match code {
            BackendErrorCode::InvalidRequest => "the browser test request is invalid",
            BackendErrorCode::PreconditionFailed => {
                "the Playwright browser preconditions are not satisfied"
            }
            _ => "the Playwright browser operation failed",
        }
        .to_string(),
        backend: Some(backend),
        capability: Some(capability),
        reason: None,
        retryable,
        diagnostic_ref: None,
    }
}

fn validate_session_id(value: &str) -> Result<(), String> {
    let Some(suffix) = value.strip_prefix("session_") else {
        return Err("invalid browser session identity".to_string());
    };
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid browser session identity".to_string());
    }
    Ok(())
}

fn validate_artifact_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 120
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("invalid browser artifact name".to_string());
    }
    Ok(())
}

fn direct_child(parent: &Path, component: &str) -> Result<PathBuf, String> {
    let path = Path::new(component);
    if component.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err("invalid browser cache component".to_string());
    }
    Ok(parent.join(component))
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if parent.exists() {
            ensure_direct_directory(parent, "browser cache parent")?;
        }
    }
    fs::create_dir_all(path).map_err(|error| format!("could not create browser cache: {error}"))?;
    ensure_direct_directory(path, "browser cache")?;
    set_directory_permissions(path)
}

fn open_store_lock(path: &Path) -> Result<File, String> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("the browser artifact lock must not be a symlink".to_string());
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("could not open the browser artifact lock: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| format!("could not inspect the browser artifact lock: {error}"))?
        .is_file()
    {
        return Err("the browser artifact lock must be a direct file".to_string());
    }
    set_file_permissions(path)?;
    Ok(file)
}

fn ensure_direct_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {label}: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("the {label} must be a direct directory"));
    }
    Ok(())
}

fn write_json_private<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        return Err("the browser index already exists".to_string());
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not create browser index: {error}"))?;
    set_file_permissions(path)?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("could not serialize browser index: {error}"))?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not persist browser index: {error}"))
}

fn read_json_bounded<T: for<'de> Deserialize<'de>>(path: &Path, maximum: u64) -> Result<T, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect browser record: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum {
        return Err("browser record is not a bounded direct file".to_string());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(maximum + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("could not read browser record: {error}"))?;
    if bytes.len() as u64 > maximum {
        return Err("browser record exceeds its size limit".to_string());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse browser record: {error}"))
}

fn directory_size(path: &Path) -> Result<u64, String> {
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|error| format!("could not size browser run: {error}"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("could not inspect browser run: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("browser run contains a symlink".to_string());
        }
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn random_suffix() -> Result<String, String> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random)
        .map_err(|error| format!("could not generate browser cache nonce: {error}"))?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not protect browser cache: {error}"))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not protect browser artifact: {error}"))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{BrowserRuntimeContext, BrowserTestPolicy, PlatformId, RuntimeMode};
    use serde_json::json;

    fn request(suite_path: &Path) -> BrowserTestRequest {
        BrowserTestRequest {
            session_id: format!("session_{}", "ab".repeat(16)),
            base_url: "http://127.0.0.1:8080".to_string(),
            suite_path: suite_path.to_string_lossy().to_string(),
            runtime_context: BrowserRuntimeContext {
                host_platform: PlatformId::Linux,
                studio_platform: PlatformId::Windows,
                runtime_platform: Some(PlatformId::Linux),
                backend: BackendId::LinuxWinboat,
                runtime_mode: RuntimeMode::Portable,
                studio_version: None,
                runtime_version: Some("11.12.2".to_string()),
            },
            policy: BrowserTestPolicy {
                navigation_timeout_milliseconds: 30_000,
                action_timeout_milliseconds: 10_000,
                assertion_timeout_milliseconds: 5_000,
                fail_on_console_error: true,
                fail_on_network_failure: true,
                record_video: false,
                record_har: false,
                max_artifact_bytes: 128 * 1024 * 1024,
                retention_runs: 20,
            },
        }
    }

    #[test]
    fn suite_reader_rejects_symlinks_and_unbounded_files() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let suite = temporary.path().join("suite.json");
        fs::write(&suite, br#"{"schemaVersion":"1.0.0"}"#).expect("suite");
        assert!(read_suite(&suite).is_ok());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = temporary.path().join("link.json");
            symlink(&suite, &link).expect("suite symlink");
            assert!(read_suite(&link).is_err());
        }
    }

    #[test]
    fn policy_and_session_validation_are_bounded() {
        let request = request(Path::new("suite.json"));
        assert!(validate_policy(&request).is_ok());
        assert!(validate_session_id(&request.session_id).is_ok());
        assert!(validate_session_id("session_short").is_err());
        assert!(validate_session_id(&format!("session_{}", "A".repeat(32))).is_err());
        assert!(validate_artifact_name("test-001-trace.zip").is_ok());
        assert!(validate_artifact_name("../trace.zip").is_err());
    }

    #[test]
    fn doctor_contract_rejects_inconsistent_readiness() {
        let value = json!({
            "schemaVersion": CONTRACT_SCHEMA_VERSION,
            "runnerVersion": RUNNER_VERSION,
            "ready": true,
            "nodeVersion": MINIMUM_NODE_VERSION,
            "minimumNodeVersion": MINIMUM_NODE_VERSION,
            "nodeSupported": true,
            "playwrightVersion": "1.62.1",
            "chromium": {
                "installed": true,
                "launchable": true,
                "version": "151.0.7922.34"
            },
            "downloadPolicy": "explicit-only"
        });
        assert!(parse_doctor(value.clone(), BackendId::LinuxWinboat).is_ok());

        let mut inconsistent = value;
        inconsistent["nodeSupported"] = Value::Bool(false);
        assert!(parse_doctor(inconsistent, BackendId::LinuxWinboat).is_err());

        let mut unsupported = json!({
            "schemaVersion": CONTRACT_SCHEMA_VERSION,
            "runnerVersion": RUNNER_VERSION,
            "ready": false,
            "nodeVersion": "22.22.1",
            "minimumNodeVersion": MINIMUM_NODE_VERSION,
            "nodeSupported": true,
            "playwrightVersion": "1.62.1",
            "chromium": { "installed": false, "launchable": false },
            "downloadPolicy": "explicit-only"
        });
        assert!(parse_doctor(unsupported.clone(), BackendId::LinuxWinboat).is_err());
        unsupported["nodeSupported"] = Value::Bool(false);
        assert!(parse_doctor(unsupported, BackendId::LinuxWinboat).is_ok());
    }

    #[test]
    fn secret_collection_honors_sensitive_flags_and_encodings() {
        const SECRET: &str = "p@ss \"word\"</with+symbols&";
        const COOKIE: &str = "storage-cookie/private+value";
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("storage-state.json");
        fs::write(
            &storage,
            serde_json::to_vec(&json!({
                "cookies": [{"name": "auth", "value": COOKIE}],
                "origins": []
            }))
            .expect("storage JSON"),
        )
        .expect("storage state");
        std::env::set_var("MENDIMARU_TEST_PASSWORD", SECRET);
        std::env::set_var("MENDIMARU_TEST_USERNAME", "public-user");
        std::env::set_var("MENDIMARU_TEST_STORAGE_STATE", &storage);
        let suite = json!({
            "secretEnv": ["MENDIMARU_TEST_PASSWORD"],
            "storageStateEnv": "MENDIMARU_TEST_STORAGE_STATE",
            "steps": [
                {"valueFromEnv": "MENDIMARU_TEST_PASSWORD"},
                {"valueFromEnv": "MENDIMARU_TEST_USERNAME", "sensitive": false}
            ]
        });
        let values = secret_values(&suite).expect("secret values");
        assert!(values.iter().any(|value| value == SECRET.as_bytes()));
        assert!(values
            .iter()
            .any(|value| String::from_utf8_lossy(value).contains("%40")));
        assert!(values
            .iter()
            .any(|value| String::from_utf8_lossy(value).contains("%2f")));
        assert!(values
            .iter()
            .any(|value| String::from_utf8_lossy(value).contains("&lt;")));
        assert!(values
            .iter()
            .any(|value| String::from_utf8_lossy(value).contains("\\\"word\\\"")));
        use base64::Engine;
        let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(SECRET);
        assert!(values.iter().any(|value| value == url_safe.as_bytes()));
        assert!(values.iter().any(|value| value == COOKIE.as_bytes()));
        assert!(!values.iter().any(|value| value == b"public-user"));
        std::env::remove_var("MENDIMARU_TEST_PASSWORD");
        std::env::remove_var("MENDIMARU_TEST_STORAGE_STATE");
        std::env::remove_var("MENDIMARU_TEST_USERNAME");
    }

    #[test]
    fn private_store_rejects_tampered_artifacts() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory = temporary.path().join("run");
        ensure_private_directory(&directory).expect("private run");
        let filename = directory.join("report.html");
        fs::write(&filename, b"safe").expect("artifact");
        let (digest, size) = digest_file(&filename).expect("digest");
        let mut descriptor = ArtifactDescriptor::create(
            format!("session_{}", "cd".repeat(16)),
            BackendId::LinuxWinboat,
            ArtifactKind::BrowserReport,
        )
        .expect("descriptor");
        descriptor.location = Some(format!("mendimaru-cache://{}", descriptor.artifact_id));
        descriptor.sha256 = Some(digest);
        descriptor.size_bytes = Some(size);
        let record = ArtifactRecord {
            descriptor,
            relative_path: "report.html".to_string(),
        };
        assert!(verify_artifact(&directory, &record).is_ok());
        fs::write(&filename, b"tampered").expect("tamper artifact");
        assert!(verify_artifact(&directory, &record).is_err());
    }

    #[test]
    fn artifact_inventory_enforces_the_request_limit_independently() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let filename = temporary.path().join("report.html");
        fs::write(&filename, vec![b'x'; 1_025]).expect("oversized artifact");
        let files = [RunnerArtifact {
            path: "report.html".to_string(),
            kind: ArtifactKind::BrowserReport,
            media_type: "text/html; charset=utf-8".to_string(),
        }];
        assert!(materialize_artifacts(
            temporary.path(),
            &files,
            &format!("session_{}", "ef".repeat(16)),
            BackendId::LinuxWinboat,
            1_024,
        )
        .is_err());
    }

    #[test]
    fn artifact_store_lock_serializes_commit_prune_and_lookup() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("store.lock");
        let first = open_store_lock(&path).expect("first store lock");
        let second = open_store_lock(&path).expect("second store lock");

        fs2::FileExt::lock_exclusive(&first).expect("acquire first lock");
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_err());
        fs2::FileExt::unlock(&first).expect("release first lock");
        fs2::FileExt::lock_exclusive(&second).expect("acquire second lock");
    }
}
