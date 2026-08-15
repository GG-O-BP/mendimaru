use crate::app_paths::AppPaths;
use crate::contracts::{
    BackendError, BackendErrorCode, CapabilityId, CapabilityLimitation, RuntimeBuildRequest,
    RuntimeBuildResult, RuntimeLogBatch, RuntimeMode, RuntimeStartRequest, RuntimeStatus,
    StudioSessionStatus,
};
use crate::downloads::{DownloadManager, InstallError};
use crate::models::{
    AppConfig, CommandError, CommandErrorCode, DownloadProgress, DownloadState,
    EnvironmentDiagnosticAction, EnvironmentDiagnosticId, EnvironmentDiagnosticStatus,
    EnvironmentStatus, OperationKind, OperationRecord, OperationStage, StudioVersion,
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
    project_id: &str,
    clean: bool,
    operation_timeout: Duration,
) -> ApplicationResult<(RuntimeBuildResult, RuntimeStatus)> {
    const SUPERVISOR_RESPONSE_MARGIN: Duration = Duration::from_secs(5);
    let started = Instant::now();
    let build = runtime_build(config, project_id, clean).await?;
    let readiness_timeout = operation_timeout
        .checked_sub(started.elapsed())
        .and_then(|remaining| remaining.checked_sub(SUPERVISOR_RESPONSE_MARGIN))
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::OperationFailed,
                "the Runtime build exhausted the operation timeout".to_string(),
            )
        })?;
    let request = RuntimeStartRequest {
        session_id: build.session_id.clone(),
        mode: RuntimeMode::Portable,
        package_artifact_id: Some(build.package_artifact.artifact_id.clone()),
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

pub(crate) async fn launch(
    paths: &AppPaths,
    config: &AppConfig,
    version: String,
    project_mpr_path: Option<&str>,
    retry_of: Option<String>,
) -> ApplicationResult<String> {
    crate::platform::validate_version(&version)
        .map_err(|_| invalid_request("the Studio Pro version is invalid"))?;
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
    match result {
        Ok(()) => tracker.succeed().map_err(operation_history_error)?,
        Err(InstallError::Cancelled(message)) => {
            let command_error = CommandError::new(CommandErrorCode::DownloadCancelled, message);
            let _ = tracker.cancel("download_cancelled");
            return Err(command_error);
        }
        Err(error) => {
            let command_error = install_error(error);
            let _ = tracker.fail(&command_error);
            return Err(command_error);
        }
    }
    Ok(operation_id)
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
    use super::{exact_project_launch_path, launch_project, operation, project, projects};
    use crate::app_paths::AppPaths;
    use crate::models::{AppConfig, CommandErrorCode, ContainerRuntime};
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
