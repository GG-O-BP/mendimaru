use crate::app_paths::AppPaths;
use crate::contracts::{
    ArtifactDescriptor, BackendError, BackendErrorCode, BackendId, BrowserRuntimeContext,
    BrowserTestPolicy, BrowserTestRequest, BrowserTestSummary, CapabilityId, CapabilityLimitation,
    RuntimeBuildRequest, RuntimeBuildResult, RuntimeLogBatch, RuntimeMode, RuntimeStartRequest,
    RuntimeStatus, StudioSessionStatus,
};
use crate::downloads::{DownloadManager, InstallError};
use crate::models::{
    AppConfig, CommandError, CommandErrorCode, DownloadProgress, DownloadState,
    EnvironmentDiagnosticAction, EnvironmentDiagnosticErrorCode, EnvironmentDiagnosticId,
    EnvironmentDiagnosticStatus, EnvironmentStatus, OperationKind, OperationRecord, OperationStage,
    StudioVersion,
};
use crate::operations::{OperationTracker, SessionActionGuard};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) type ApplicationResult<T> = Result<T, CommandError>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeStudioVersion {
    pub version: String,
    pub removable: bool,
}

impl From<StudioVersion> for SafeStudioVersion {
    fn from(value: StudioVersion) -> Self {
        Self {
            version: value.version,
            removable: value.removable,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeProject {
    pub project_id: String,
    pub name: String,
    pub required_version: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeEnvironmentStatus {
    pub ready: bool,
    pub container_status: crate::models::ContainerStatus,
    pub checks: Vec<SafeEnvironmentCheck>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeEnvironmentCheck {
    pub id: EnvironmentDiagnosticId,
    pub status: EnvironmentDiagnosticStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<EnvironmentDiagnosticAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<EnvironmentDiagnosticErrorCode>,
}

impl From<&EnvironmentStatus> for SafeEnvironmentStatus {
    fn from(value: &EnvironmentStatus) -> Self {
        Self {
            ready: value.ready,
            container_status: value.container_status,
            checks: value
                .diagnostics
                .iter()
                .map(|diagnostic| SafeEnvironmentCheck {
                    id: diagnostic.id,
                    status: diagnostic.status,
                    action: diagnostic.action,
                    error_code: diagnostic.error_code,
                })
                .collect(),
        }
    }
}

pub(crate) fn load_config(paths: &AppPaths) -> ApplicationResult<AppConfig> {
    crate::config::load_config_from(paths)
        .map_err(|message| CommandError::new(CommandErrorCode::ConfigLoadFailed, message))
}

pub(crate) async fn environment_status(config: &AppConfig) -> SafeEnvironmentStatus {
    SafeEnvironmentStatus::from(&crate::platform::environment_status(config).await)
}

pub(crate) async fn ensure_environment(
    config: &AppConfig,
    timeout: Duration,
) -> ApplicationResult<SafeEnvironmentStatus> {
    let initial = crate::platform::environment_status(config).await;
    if initial.ready {
        return Ok(SafeEnvironmentStatus::from(&initial));
    }

    #[cfg(target_os = "linux")]
    {
        if !initial.winboat_available || !initial.compose_available || !initial.runtime_available {
            return Err(precondition_error(
                CapabilityId::StudioDetect,
                "the WinBoat environment prerequisites are not ready",
                false,
            ));
        }
        crate::winboat::start_container(config).await.map_err(|_| {
            precondition_error(
                CapabilityId::StudioDetect,
                "the WinBoat environment could not be started",
                true,
            )
        })?;
        let started = tokio::time::Instant::now();
        while started.elapsed() < timeout {
            let current = crate::platform::environment_status(config).await;
            if current.ready {
                return Ok(SafeEnvironmentStatus::from(&current));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Err(precondition_error(
            CapabilityId::StudioDetect,
            "the WinBoat environment did not become ready before the timeout",
            true,
        ))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = timeout;
        Err(precondition_error(
            CapabilityId::StudioDetect,
            "the native environment is not ready",
            false,
        ))
    }
}

pub(crate) async fn installed_versions(
    config: &AppConfig,
) -> ApplicationResult<Vec<SafeStudioVersion>> {
    Ok(crate::platform::installed_versions(config)
        .await?
        .into_iter()
        .map(SafeStudioVersion::from)
        .collect())
}

pub(crate) fn projects(config: &AppConfig) -> ApplicationResult<Vec<SafeProject>> {
    crate::projects::scan_projects(config)?
        .iter()
        .map(safe_project)
        .collect::<Result<Vec<_>, _>>()
        .map_err(CommandError::from)
}

pub(crate) fn project(config: &AppConfig, project_id: &str) -> ApplicationResult<SafeProject> {
    let project = resolve_project(config, project_id)?;
    safe_project(&project).map_err(CommandError::from)
}

pub(crate) async fn studio_sessions(
    config: &AppConfig,
) -> ApplicationResult<Vec<StudioSessionStatus>> {
    Ok(crate::platform::studio_sessions(config).await?)
}

pub(crate) async fn studio_session(
    config: &AppConfig,
    session_id: &str,
) -> ApplicationResult<StudioSessionStatus> {
    Ok(crate::platform::studio_session_status(config, session_id).await?)
}

pub(crate) async fn reconnect_session(
    paths: &AppPaths,
    config: &AppConfig,
    session_id: &str,
) -> ApplicationResult<()> {
    let session = crate::platform::studio_session_status(config, session_id).await?;
    let _guard = SessionActionGuard::begin_with_paths(paths, config, &session.version)
        .map_err(session_conflict_error)?;
    Ok(crate::platform::reconnect_studio_session(config, session_id).await?)
}

pub(crate) async fn stop_session(
    paths: &AppPaths,
    config: &AppConfig,
    session_id: &str,
) -> ApplicationResult<()> {
    let session = crate::platform::studio_session_status(config, session_id).await?;
    let _guard = SessionActionGuard::begin_with_paths(paths, config, &session.version)
        .map_err(session_conflict_error)?;
    Ok(crate::platform::stop_studio_session(config, session_id).await?)
}

pub(crate) async fn runtime_build(
    config: &AppConfig,
    project_id: &str,
    clean: bool,
) -> ApplicationResult<RuntimeBuildResult> {
    let project = resolve_project(config, project_id)?;
    let required_version = project.version.clone().ok_or_else(|| {
        precondition_error(
            CapabilityId::RuntimeBuild,
            "the project does not declare one unambiguous exact Mendix version",
            false,
        )
    })?;
    let request = RuntimeBuildRequest {
        session_id: crate::contracts::secure_identifier("session")?,
        project_path: project.mpr_path,
        required_version,
        clean,
    };
    crate::platform::build_runtime(config, &request)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn runtime_start(
    config: &AppConfig,
    project_id: Option<&str>,
    clean: bool,
    mode: RuntimeMode,
    studio_session_id: Option<&str>,
    guest_port: Option<u16>,
    operation_timeout: Duration,
) -> ApplicationResult<(Option<RuntimeBuildResult>, RuntimeStatus)> {
    const SUPERVISOR_RESPONSE_MARGIN: Duration = Duration::from_secs(5);
    let started = Instant::now();
    let (build, session_id, package_artifact_id) = match mode {
        RuntimeMode::Portable => {
            let project_id = project_id.ok_or_else(|| {
                invalid_request("--project-id is required for portable Runtime mode")
            })?;
            if studio_session_id.is_some() || guest_port.is_some() {
                return Err(invalid_request(
                    "Studio session and guest port options require studio-run-locally mode",
                ));
            }
            let build = runtime_build(config, project_id, clean).await?;
            let session_id = build.session_id.clone();
            let artifact_id = build.package_artifact.artifact_id.clone();
            (Some(build), session_id, Some(artifact_id))
        }
        RuntimeMode::StudioRunLocally => {
            if project_id.is_some() || clean {
                return Err(invalid_request(
                    "project build options are not accepted in studio-run-locally mode",
                ));
            }
            (None, crate::contracts::secure_identifier("session")?, None)
        }
        RuntimeMode::ExternalUrl => {
            return Err(invalid_request(
                "external-url Runtime sessions cannot be started by this command",
            ));
        }
    };
    let readiness_timeout = operation_timeout
        .checked_sub(started.elapsed())
        .and_then(|remaining| {
            if mode == RuntimeMode::Portable {
                remaining.checked_sub(SUPERVISOR_RESPONSE_MARGIN)
            } else {
                Some(remaining)
            }
        })
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::OperationFailed,
                "the Runtime preparation exhausted the operation timeout".to_string(),
            )
        })?;
    let request = RuntimeStartRequest {
        session_id,
        mode,
        package_artifact_id,
        studio_session_id: studio_session_id.map(ToString::to_string),
        guest_port,
        readiness_timeout_seconds: readiness_timeout.as_secs().clamp(1, 3_600),
    };
    let status = crate::platform::start_runtime(config, &request)
        .await
        .map_err(CommandError::from)?;
    Ok((build, status))
}

pub(crate) async fn runtime_status(
    config: &AppConfig,
    session_id: &str,
) -> ApplicationResult<RuntimeStatus> {
    crate::platform::runtime_status(config, session_id)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn runtime_wait(
    config: &AppConfig,
    session_id: &str,
) -> ApplicationResult<RuntimeStatus> {
    crate::platform::wait_runtime(config, session_id)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn runtime_url(config: &AppConfig, session_id: &str) -> ApplicationResult<String> {
    crate::platform::runtime_url(config, session_id)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn runtime_stop(config: &AppConfig, session_id: &str) -> ApplicationResult<()> {
    crate::platform::stop_runtime(config, session_id)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn runtime_logs(
    config: &AppConfig,
    session_id: &str,
    cursor: Option<&str>,
) -> ApplicationResult<RuntimeLogBatch> {
    crate::platform::runtime_logs(config, session_id, cursor)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn browser_doctor(
    backend: BackendId,
) -> ApplicationResult<crate::browser::BrowserDoctor> {
    crate::browser::doctor(backend)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn browser_install_chromium(
    backend: BackendId,
) -> ApplicationResult<crate::browser::BrowserDoctor> {
    crate::browser::install_chromium(backend)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn browser_test_url(
    backend: BackendId,
    base_url: &str,
    suite_path: &str,
    policy: BrowserTestPolicy,
) -> ApplicationResult<BrowserTestSummary> {
    let manifest =
        crate::platform::capability_manifest(Some(backend)).map_err(CommandError::from)?;
    let request = BrowserTestRequest {
        session_id: crate::contracts::secure_identifier("session")?,
        base_url: normalize_browser_url(base_url)?,
        suite_path: suite_path.to_string(),
        runtime_context: BrowserRuntimeContext {
            host_platform: manifest.host_platform,
            studio_platform: manifest.studio_platform,
            runtime_platform: None,
            backend: manifest.backend,
            runtime_mode: RuntimeMode::ExternalUrl,
            studio_version: None,
            runtime_version: None,
        },
        policy,
    };
    crate::browser::test(&request, backend)
        .await
        .map_err(CommandError::from)
}

pub(crate) async fn browser_test_runtime(
    config: &AppConfig,
    runtime_session_id: &str,
    suite_path: &str,
    policy: BrowserTestPolicy,
) -> ApplicationResult<BrowserTestSummary> {
    let manifest = crate::platform::capability_manifest(None).map_err(CommandError::from)?;
    let status = runtime_status(config, runtime_session_id).await?;
    if !status.http_ready {
        return Err(precondition_error(
            CapabilityId::BrowserTest,
            "the Runtime session is not HTTP-ready",
            true,
        ));
    }
    let base_url = runtime_url(config, runtime_session_id).await?;
    let runtime_platform = match status.mode {
        RuntimeMode::Portable => Some(manifest.host_platform),
        RuntimeMode::StudioRunLocally => Some(manifest.studio_platform),
        RuntimeMode::ExternalUrl => None,
    };
    let studio_version = if let Some(session_id) = status.studio_session_id.as_deref() {
        studio_session(config, session_id)
            .await
            .ok()
            .map(|session| session.version)
    } else {
        None
    };
    let runtime_version = status.runtime_version.clone().or_else(|| {
        (status.mode == RuntimeMode::StudioRunLocally)
            .then(|| studio_version.clone())
            .flatten()
    });
    let request = BrowserTestRequest {
        session_id: crate::contracts::secure_identifier("session")?,
        base_url,
        suite_path: suite_path.to_string(),
        runtime_context: BrowserRuntimeContext {
            host_platform: manifest.host_platform,
            studio_platform: manifest.studio_platform,
            runtime_platform,
            backend: manifest.backend,
            runtime_mode: status.mode,
            studio_version,
            runtime_version,
        },
        policy,
    };
    crate::platform::run_browser_test(config, &request)
        .await
        .map_err(CommandError::from)
}

pub(crate) fn browser_artifacts(
    backend: BackendId,
    session_id: &str,
) -> ApplicationResult<Vec<ArtifactDescriptor>> {
    crate::browser::artifacts(session_id, backend).map_err(CommandError::from)
}

fn normalize_browser_url(value: &str) -> ApplicationResult<String> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| invalid_request("the browser base URL is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_request("the browser base URL is unsafe"));
    }
    Ok(url.to_string())
}

pub(crate) async fn launch(
    paths: &AppPaths,
    config: &AppConfig,
    version: String,
    project_mpr_path: Option<&str>,
    retry_of: Option<String>,
) -> ApplicationResult<String> {
    crate::platform::validate_version(&version)
        .map_err(|_| invalid_request("the Studio Pro version is invalid"))?;
    ensure_no_connected_remote_app(CapabilityId::StudioStart)?;
    let protected_project = project_mpr_path.is_some();
    let tracker = OperationTracker::begin_with_paths(
        paths,
        config,
        OperationKind::Launch,
        &version,
        protected_project,
        OperationStage::Launching,
        retry_of,
    )
    .map_err(operation_history_error)?;
    let operation_id = tracker.id().to_string();
    let result = crate::platform::launch_studio(config, &version, &operation_id, project_mpr_path)
        .await
        .map_err(CommandError::from);
    complete_operation(tracker, result)?;
    Ok(operation_id)
}

pub(crate) async fn launch_project(
    paths: &AppPaths,
    config: &AppConfig,
    version: String,
    project_id: &str,
) -> ApplicationResult<String> {
    crate::platform::validate_version(&version)
        .map_err(|_| invalid_request("the Studio Pro version is invalid"))?;
    let project_mpr_path = exact_project_launch_path(config, project_id, &version)?;
    launch(paths, config, version, Some(&project_mpr_path), None).await
}

fn exact_project_launch_path(
    config: &AppConfig,
    project_id: &str,
    version: &str,
) -> ApplicationResult<String> {
    let project = resolve_project(config, project_id)?;
    let required = project.version.as_deref().ok_or_else(|| {
        precondition_error(
            CapabilityId::StudioStart,
            "the project does not declare one unambiguous Studio Pro version",
            false,
        )
    })?;
    if required != version {
        return Err(precondition_error(
            CapabilityId::StudioStart,
            "the requested Studio Pro version does not exactly match the project",
            false,
        ));
    }
    // The platform adapter owns host-to-Studio path conversion. Passing the
    // already converted Windows path here would bypass its workspace identity
    // check on Linux and duplicate platform policy in the common service.
    Ok(project.mpr_path)
}

pub(crate) async fn uninstall(
    paths: &AppPaths,
    config: &AppConfig,
    version: String,
    retry_of: Option<String>,
) -> ApplicationResult<String> {
    crate::platform::validate_version(&version)
        .map_err(|_| invalid_request("the Studio Pro version is invalid"))?;
    ensure_no_connected_remote_app(CapabilityId::StudioUninstall)?;
    ensure_no_running_session(config, &version, CapabilityId::StudioUninstall).await?;
    let tracker = OperationTracker::begin_with_paths(
        paths,
        config,
        OperationKind::Uninstall,
        &version,
        false,
        OperationStage::Uninstalling,
        retry_of,
    )
    .map_err(operation_history_error)?;
    let operation_id = tracker.id().to_string();
    let result = crate::platform::uninstall_studio(config, &version, &operation_id)
        .await
        .map_err(CommandError::from);
    invalidate_installed_versions_cache_after_mutation(paths, result.is_ok());
    complete_operation(tracker, result)?;
    Ok(operation_id)
}

pub(crate) async fn install<F>(
    paths: &AppPaths,
    config: &AppConfig,
    manager: &DownloadManager,
    version: String,
    force_redownload: bool,
    retry_of: Option<String>,
    mut on_progress: F,
) -> ApplicationResult<String>
where
    F: FnMut(&DownloadProgress) + Send,
{
    crate::platform::validate_version(&version)
        .map_err(|_| invalid_request("the Studio Pro version is invalid"))?;
    ensure_no_connected_remote_app(CapabilityId::StudioInstall)?;
    ensure_version_not_installed(
        &crate::platform::installed_versions(config).await?,
        &version,
    )?;
    ensure_no_running_session(config, &version, CapabilityId::StudioInstall).await?;
    let mut tracker = OperationTracker::begin_with_paths(
        paths,
        config,
        OperationKind::Install,
        &version,
        false,
        OperationStage::Starting,
        retry_of,
    )
    .map_err(operation_history_error)?;
    let operation_id = tracker.id().to_string();
    let result = crate::downloads::download_and_launch(
        paths,
        config,
        manager,
        version,
        &operation_id,
        force_redownload,
        |progress| {
            let _ = tracker.progress(
                operation_stage(progress),
                progress.percentage,
                progress.estimated,
            );
            on_progress(progress);
        },
    )
    .await;
    invalidate_installed_versions_cache_after_mutation(paths, result.is_ok());
    match result {
        Ok(()) => {
            tracker.succeed().map_err(operation_history_error)?;
        }
        Err(InstallError::Cancelled(message)) => {
            let command_error = CommandError::new(CommandErrorCode::DownloadCancelled, message);
            let _ = tracker.cancel("download_cancelled");
            return Err(command_error);
        }
        Err(error) => {
            let command_error = install_error(error);
            if command_error.code == CommandErrorCode::ExternalProcessCancelled {
                let _ = tracker
                    .cancel_with_code("external_process_cancelled", "external_process_cancelled");
            } else {
                let _ = tracker.fail(&command_error);
            }
            return Err(command_error);
        }
    }
    Ok(operation_id)
}

fn invalidate_installed_versions_cache_after_mutation(paths: &AppPaths, succeeded: bool) {
    if !succeeded {
        return;
    }
    if let Err(error) = crate::studio_cache::invalidate(paths) {
        if !crate::studio_trace::enabled() {
            return;
        }
        eprintln!(
            "[studio-overview] disk-cache-invalidation failed=true error_bytes={}",
            error.len()
        );
    }
}

pub(crate) fn operations(
    paths: &AppPaths,
    config: &AppConfig,
) -> ApplicationResult<Vec<OperationRecord>> {
    crate::operations::list_with_paths(paths, config).map_err(operation_history_error)
}

pub(crate) fn operation(
    paths: &AppPaths,
    config: &AppConfig,
    operation_id: &str,
) -> ApplicationResult<OperationRecord> {
    operations(paths, config)?
        .into_iter()
        .find(|record| record.id == operation_id)
        .ok_or_else(|| invalid_request("the operation ID was not found"))
}

pub(crate) async fn retry<F>(
    paths: &AppPaths,
    config: &AppConfig,
    manager: &DownloadManager,
    operation_id: &str,
    on_progress: F,
) -> ApplicationResult<String>
where
    F: FnMut(&DownloadProgress) + Send,
{
    let source = crate::operations::retry_source_with_paths(paths, config, operation_id)
        .map_err(operation_history_error)?;
    let retry_of = Some(source.id.clone());
    match source.kind {
        OperationKind::Install => {
            install(
                paths,
                config,
                manager,
                source.target_version,
                false,
                retry_of,
                on_progress,
            )
            .await
        }
        OperationKind::Uninstall => uninstall(paths, config, source.target_version, retry_of).await,
        OperationKind::Launch if !source.protected_project => {
            launch(paths, config, source.target_version, None, retry_of).await
        }
        OperationKind::Launch => Err(invalid_request(
            "project launches cannot be retried without selecting the project again",
        )),
    }
}

fn safe_project(project: &crate::models::MendixProject) -> Result<SafeProject, String> {
    Ok(SafeProject {
        project_id: project_identifier(Path::new(&project.mpr_path))?,
        name: project.name.clone(),
        required_version: project.version.clone(),
        last_modified: project.last_modified.clone(),
    })
}

fn resolve_project(
    config: &AppConfig,
    project_id: &str,
) -> ApplicationResult<crate::models::MendixProject> {
    validate_project_identifier(project_id)?;
    crate::projects::scan_projects(config)?
        .into_iter()
        .find(|project| {
            project_identifier(Path::new(&project.mpr_path)).as_deref() == Ok(project_id)
        })
        .ok_or_else(|| invalid_request("the project ID was not found"))
}

fn project_identifier(path: &Path) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|_| "the project identity could not be resolved".to_string())?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    Ok(format!("project_{digest:x}"))
}

fn validate_project_identifier(value: &str) -> ApplicationResult<()> {
    let suffix = value
        .strip_prefix("project_")
        .filter(|suffix| suffix.len() == 64 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| invalid_request("the project ID is invalid"))?;
    if suffix.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid_request("the project ID is invalid"));
    }
    Ok(())
}

async fn ensure_no_running_session(
    config: &AppConfig,
    version: &str,
    capability: CapabilityId,
) -> ApplicationResult<()> {
    if crate::platform::studio_sessions(config)
        .await?
        .iter()
        .any(|session| session.version == version)
    {
        return Err(precondition_error(
            capability,
            "the exact Studio Pro version has a running session",
            false,
        ));
    }
    Ok(())
}

fn ensure_version_not_installed(
    installed_versions: &[StudioVersion],
    version: &str,
) -> ApplicationResult<()> {
    if installed_versions
        .iter()
        .any(|installed| installed.version == version)
    {
        return Err(precondition_error(
            CapabilityId::StudioInstall,
            &crate::tr!("error-studio-already-installed", version = version),
            false,
        ));
    }
    Ok(())
}

fn ensure_no_connected_remote_app(capability: CapabilityId) -> ApplicationResult<()> {
    #[cfg(target_os = "linux")]
    let connected_version = crate::winboat::registered_client_sessions()
        .first()
        .map(|session| session.version.clone());
    #[cfg(not(target_os = "linux"))]
    let connected_version: Option<String> = None;

    ensure_no_connected_remote_app_version(connected_version.as_deref(), capability)
}

fn ensure_no_connected_remote_app_version(
    connected_version: Option<&str>,
    capability: CapabilityId,
) -> ApplicationResult<()> {
    if let Some(version) = connected_version {
        return Err(precondition_error(
            capability,
            &crate::tr!(
                "error-studio-connected-session-blocks-operation",
                version = version
            ),
            false,
        ));
    }
    Ok(())
}

fn session_conflict_error(_message: String) -> CommandError {
    precondition_error(
        CapabilityId::StudioStatus,
        "another action for this Studio Pro version is already running",
        true,
    )
}

fn complete_operation(
    tracker: OperationTracker,
    result: ApplicationResult<()>,
) -> ApplicationResult<()> {
    match result {
        Ok(()) => tracker.succeed().map_err(operation_history_error),
        Err(error) => {
            let _ = tracker.fail(&error);
            Err(error)
        }
    }
}

fn install_error(error: InstallError) -> CommandError {
    match error {
        InstallError::Cancelled(message) => {
            CommandError::new(CommandErrorCode::DownloadCancelled, message)
        }
        InstallError::Backend(error) => error.into(),
        InstallError::Other(message) => CommandError::new(CommandErrorCode::InstallFailed, message),
    }
}

fn operation_stage(progress: &DownloadProgress) -> OperationStage {
    match progress.state {
        DownloadState::Starting => OperationStage::Starting,
        DownloadState::Preparing => OperationStage::Preparing,
        DownloadState::Checking => OperationStage::Checking,
        DownloadState::Connecting => OperationStage::Connecting,
        DownloadState::Downloading => OperationStage::Downloading,
        DownloadState::Downloaded => OperationStage::Downloaded,
        DownloadState::Ready => OperationStage::Ready,
        DownloadState::Staging => OperationStage::Staging,
        DownloadState::Installing => OperationStage::Installing,
        DownloadState::Finalizing => OperationStage::Finalizing,
        DownloadState::Verifying => OperationStage::Verifying,
        DownloadState::Installed => OperationStage::Completed,
        DownloadState::Failed | DownloadState::Cancelled => OperationStage::Interrupted,
    }
}

fn invalid_request(message: &str) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidRequest, message.to_string())
}

fn operation_history_error(message: String) -> CommandError {
    CommandError::new(CommandErrorCode::OperationFailed, message)
}

fn precondition_error(capability: CapabilityId, message: &str, retryable: bool) -> CommandError {
    let backend = crate::platform::capability_manifest(None)
        .map(|manifest| manifest.backend)
        .unwrap_or_else(|_| {
            crate::platform::backend::expected_backend(crate::contracts::PlatformId::current())
                .unwrap_or(crate::contracts::BackendId::MacNative)
        });
    BackendError::precondition(
        backend,
        capability,
        CapabilityLimitation {
            code: BackendErrorCode::PreconditionFailed,
            message: message.to_string(),
            required_permission: None,
            required_version: None,
        },
        retryable,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_no_connected_remote_app_version, ensure_version_not_installed,
        exact_project_launch_path, invalidate_installed_versions_cache_after_mutation,
        launch_project, operation, project, projects, SafeEnvironmentStatus,
    };
    use crate::app_paths::AppPaths;
    use crate::contracts::CapabilityId;
    use crate::models::{
        AppConfig, CommandErrorCode, ContainerRuntime, ContainerStatus, EnvironmentDiagnostic,
        EnvironmentDiagnosticErrorCode, EnvironmentDiagnosticId, EnvironmentDiagnosticStatus,
        EnvironmentStatus, HostPlatform, PlatformCapabilities, StudioVersion,
    };
    use crate::operations::OperationTracker;
    use std::fs;

    fn config(workspace: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "missing-winboat".into(),
            compose_file: "missing-compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:9".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 9,
            shared_directory: workspace.to_string_lossy().into_owned(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "missing-freerdp".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        }
    }

    #[test]
    fn headless_environment_status_preserves_process_codes_without_observed_details() {
        let status = EnvironmentStatus {
            platform: PlatformCapabilities {
                kind: HostPlatform::LinuxWinboat,
                architecture: "x86_64".into(),
                requires_winboat: true,
                supports_studio_management: true,
                supports_installation: true,
                supports_uninstallation: true,
                supports_projects: true,
            },
            ready: false,
            winboat_available: true,
            winboat_initialized: true,
            setup_pending: false,
            compose_available: true,
            runtime_available: false,
            freerdp_available: true,
            shared_directory_available: true,
            shared_mount_matches: true,
            container_status: ContainerStatus::Unknown,
            guest_online: false,
            diagnostics: vec![EnvironmentDiagnostic {
                id: EnvironmentDiagnosticId::ContainerRuntime,
                status: EnvironmentDiagnosticStatus::Failure,
                observed: Some("private runtime output".into()),
                action: None,
                error_code: Some(EnvironmentDiagnosticErrorCode::ExternalProcessTimeout),
            }],
        };

        let serialized = serde_json::to_string(&SafeEnvironmentStatus::from(&status))
            .expect("headless environment status serializes");
        assert!(serialized.contains("\"errorCode\":\"external-process-timeout\""));
        assert!(!serialized.contains("private runtime output"));
    }

    #[test]
    fn installation_rejects_an_exact_version_that_is_already_installed() {
        crate::i18n::initialize("en-US").expect("localization");
        let installed = StudioVersion {
            version: "11.12.3".into(),
            display_name: "Studio Pro 11.12.3".into(),
            executable_path: r"C:\Program Files\Mendix\11.12.3\modeler\StudioPro.exe".into(),
            install_root: r"C:\Program Files\Mendix\11.12.3".into(),
            source: "fixture".into(),
            removable: true,
        };
        let error = ensure_version_not_installed(&[installed], "11.12.3")
            .expect_err("duplicate install must fail");
        assert_eq!(error.code, CommandErrorCode::PreconditionFailed);
        assert!(error.message.contains("11.12.3"));
        ensure_version_not_installed(&[], "11.12.3").expect("absent version may install");
    }

    #[test]
    fn mutation_cache_is_retained_on_failure_and_invalidated_on_success() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        let config = config(temporary.path());
        let installed = StudioVersion {
            version: "11.12.3".into(),
            display_name: "Studio Pro 11.12.3".into(),
            executable_path: r"C:\Program Files\Mendix\11.12.3\modeler\StudioPro.exe".into(),
            install_root: r"C:\Program Files\Mendix\11.12.3".into(),
            source: "fixture".into(),
            removable: true,
        };
        crate::studio_cache::save(&paths, &config, std::slice::from_ref(&installed))
            .expect("save cache");

        invalidate_installed_versions_cache_after_mutation(&paths, false);
        assert_eq!(
            crate::studio_cache::load(&paths, &config)
                .expect("failed mutation retains cache")
                .versions,
            vec![installed]
        );

        invalidate_installed_versions_cache_after_mutation(&paths, true);
        assert!(crate::studio_cache::load(&paths, &config)
            .expect("successful mutation invalidates cache")
            .versions
            .is_empty());
    }

    #[test]
    fn connected_remote_app_rejects_new_mutations_as_preconditions() {
        crate::i18n::initialize("en-US").expect("localization");
        for capability in [
            CapabilityId::StudioStart,
            CapabilityId::StudioInstall,
            CapabilityId::StudioUninstall,
        ] {
            let error = ensure_no_connected_remote_app_version(Some("11.13.0"), capability)
                .expect_err("connected RemoteApp must block another mutation");
            assert_eq!(error.code, CommandErrorCode::PreconditionFailed);
            assert!(error.message.contains("11.13.0"));
            assert_eq!(
                error
                    .details
                    .as_deref()
                    .and_then(|details| details.capability),
                Some(capability)
            );
        }
        ensure_no_connected_remote_app_version(None, CapabilityId::StudioStart)
            .expect("no connected RemoteApp permits launch");
    }

    #[test]
    fn project_contract_uses_an_opaque_stable_identifier_and_no_paths() {
        crate::i18n::initialize("en-US").expect("localization");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory = temporary.path().join("Orders");
        fs::create_dir(&directory).expect("project directory");
        fs::write(directory.join("Orders.mpr"), b"mpr fixture").expect("mpr fixture");
        fs::write(
            directory.join("project-settings.user.json"),
            r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.12.2.0"}]}"#,
        )
        .expect("settings fixture");
        let config = config(temporary.path());

        let listed = projects(&config).expect("project list");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].project_id.starts_with("project_"));
        assert_eq!(listed[0].project_id.len(), 72);
        assert_eq!(
            project(&config, &listed[0].project_id).expect("project lookup"),
            listed[0]
        );
        let serialized = serde_json::to_string(&listed).expect("serialize projects");
        assert!(!serialized.contains(temporary.path().to_string_lossy().as_ref()));
        assert!(!serialized.contains("mprPath"));
        assert!(!serialized.contains("windowsPath"));
    }

    #[test]
    fn mismatched_project_version_fails_before_any_backend_or_file_mutation() {
        crate::i18n::initialize("en-US").expect("localization");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory = temporary.path().join("Orders");
        fs::create_dir(&directory).expect("project directory");
        let mpr = directory.join("Orders.mpr");
        fs::write(&mpr, b"immutable mpr fixture").expect("mpr fixture");
        fs::write(
            directory.join("project-settings.user.json"),
            r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.12.2.0"}]}"#,
        )
        .expect("settings fixture");
        let config = config(temporary.path());
        let project_id = projects(&config).expect("projects")[0].project_id.clone();
        let before = fs::read(&mpr).expect("read before");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );

        let error = tauri::async_runtime::block_on(launch_project(
            &paths,
            &config,
            "11.13.0".to_string(),
            &project_id,
        ))
        .expect_err("mismatch must fail");
        assert_eq!(error.code, CommandErrorCode::PreconditionFailed);
        assert_eq!(fs::read(&mpr).expect("read after"), before);
        assert!(!paths
            .config_directory()
            .join("operation-history.json")
            .exists());
    }

    #[test]
    fn interrupted_shared_service_operations_are_requeryable_by_id() {
        crate::i18n::initialize("en-US").expect("localization");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let config = config(temporary.path());
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        let tracker = OperationTracker::begin_with_paths(
            &paths,
            &config,
            crate::models::OperationKind::Launch,
            "11.12.2",
            false,
            crate::models::OperationStage::Launching,
            None,
        )
        .expect("begin operation");
        let operation_id = tracker.id().to_string();
        drop(tracker);

        let restored = operation(&paths, &config, &operation_id).expect("requery interruption");
        assert_eq!(restored.id, operation_id);
        assert_eq!(restored.state, crate::models::OperationState::Interrupted);
        assert!(restored.retryable);
    }

    #[test]
    fn exact_project_launch_keeps_the_host_path_for_the_platform_adapter() {
        crate::i18n::initialize("en-US").expect("localization");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory = temporary.path().join("Orders");
        fs::create_dir(&directory).expect("project directory");
        let mpr = directory.join("Orders.mpr");
        fs::write(&mpr, b"mpr fixture").expect("mpr fixture");
        fs::write(
            directory.join("project-settings.user.json"),
            r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.12.2.0"}]}"#,
        )
        .expect("settings fixture");
        let config = config(temporary.path());
        let project_id = projects(&config).expect("projects")[0].project_id.clone();

        let selected =
            exact_project_launch_path(&config, &project_id, "11.12.2").expect("exact project path");
        assert_eq!(std::path::Path::new(&selected), mpr);
        assert_ne!(selected, r"\\host.lan\Data\Orders\Orders.mpr");
    }
}
