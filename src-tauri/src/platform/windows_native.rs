mod discovery;
mod process;
mod security;
mod sessions;

use crate::contracts::StudioSessionStatus;
use crate::models::{
    AppConfig, ContainerStatus, EnvironmentDiagnostic, EnvironmentDiagnosticAction,
    EnvironmentDiagnosticId, EnvironmentDiagnosticStatus, EnvironmentStatus, StudioInstallPhase,
    StudioInstallProgress, StudioVersion,
};
use crate::process::{CancellationToken, CommandFailure, CommandFailureKind, CommandPolicy};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const INSTALL_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const UNINSTALL_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const INSTALLER_PROCESS_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const UNINSTALLER_PROCESS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const INSTALL_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const INSTALLER_PROCESS_POLICY: CommandPolicy = CommandPolicy::new(INSTALLER_PROCESS_TIMEOUT, 0);
const UNINSTALLER_PROCESS_POLICY: CommandPolicy =
    CommandPolicy::new(UNINSTALLER_PROCESS_TIMEOUT, 0);
const SUCCESS_EXIT_CODES: [u32; 3] = [0, 1641, 3010];

#[derive(Debug)]
pub(super) struct NativeOperationFailure {
    pub(super) message: String,
    pub(super) failure_kind: Option<CommandFailureKind>,
}

impl NativeOperationFailure {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            failure_kind: None,
        }
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.message.is_empty()
    }
}

impl From<String> for NativeOperationFailure {
    fn from(message: String) -> Self {
        Self::message(message)
    }
}

impl From<&str> for NativeOperationFailure {
    fn from(message: &str) -> Self {
        Self::message(message)
    }
}

impl From<CommandFailure> for NativeOperationFailure {
    fn from(error: CommandFailure) -> Self {
        let failure_kind = Some(error.kind());
        Self {
            message: error.to_string(),
            failure_kind,
        }
    }
}

impl fmt::Display for NativeOperationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NativeOperationFailure {}

impl PartialEq<&str> for NativeOperationFailure {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

#[derive(Clone, Copy)]
struct LifecycleTiming {
    timeout: Duration,
    poll_interval: Duration,
}

const INSTALL_TIMING: LifecycleTiming = LifecycleTiming {
    timeout: INSTALL_VERIFICATION_TIMEOUT,
    poll_interval: Duration::from_secs(2),
};
const UNINSTALL_TIMING: LifecycleTiming = LifecycleTiming {
    timeout: UNINSTALL_VERIFICATION_TIMEOUT,
    poll_interval: Duration::from_secs(2),
};

struct InstallLifecycleHooks<V, R, D> {
    verify: V,
    run_elevated: R,
    find_installed: D,
    timing: LifecycleTiming,
}

#[derive(Debug, PartialEq, Eq)]
struct StudioLaunchRequest {
    executable: PathBuf,
    project: Option<PathBuf>,
}

pub(super) fn environment_status(config: &AppConfig) -> EnvironmentStatus {
    let platform = super::capabilities();
    let shared_directory_available = Path::new(&config.shared_directory).is_dir();
    let browser_available = crate::marketplace::browser_executable().is_some();
    let ready = platform.supports_studio_management && shared_directory_available;
    EnvironmentStatus {
        platform,
        ready,
        winboat_available: false,
        winboat_initialized: false,
        setup_pending: false,
        compose_available: false,
        runtime_available: true,
        freerdp_available: false,
        shared_directory_available,
        shared_mount_matches: shared_directory_available,
        container_status: ContainerStatus::NotFound,
        guest_online: ready,
        diagnostics: vec![
            EnvironmentDiagnostic {
                id: EnvironmentDiagnosticId::SharedDirectory,
                status: if shared_directory_available {
                    EnvironmentDiagnosticStatus::Success
                } else {
                    EnvironmentDiagnosticStatus::Failure
                },
                observed: None,
                action: (!shared_directory_available)
                    .then_some(EnvironmentDiagnosticAction::OpenSettings),
                error_code: None,
            },
            EnvironmentDiagnostic {
                id: EnvironmentDiagnosticId::MarketplaceBrowser,
                status: if browser_available {
                    EnvironmentDiagnosticStatus::Success
                } else {
                    EnvironmentDiagnosticStatus::Warning
                },
                observed: None,
                action: (!browser_available).then_some(EnvironmentDiagnosticAction::Redetect),
                error_code: None,
            },
        ],
    }
}

pub(super) fn installed_versions(config: &AppConfig) -> Result<Vec<StudioVersion>, String> {
    ensure_supported()?;
    Ok(discovery::discover(config)
        .into_iter()
        .map(|record| record.studio)
        .collect())
}

pub(super) fn studio_sessions(config: &AppConfig) -> Result<Vec<StudioSessionStatus>, String> {
    ensure_supported()?;
    sessions::list(config)
}

pub(super) fn reconnect_studio_session(config: &AppConfig, session_id: &str) -> Result<(), String> {
    ensure_supported()?;
    sessions::reconnect(config, session_id)
}

pub(super) fn stop_studio_session(config: &AppConfig, session_id: &str) -> Result<(), String> {
    ensure_supported()?;
    sessions::stop(config, session_id)
}

pub(super) fn verify_installer(path: &Path) -> Result<String, String> {
    ensure_supported()?;
    security::verify_mendix_executable(path)
}

pub(super) fn launch_studio(
    config: &AppConfig,
    version: &str,
    project_mpr_path: Option<&str>,
) -> Result<(), String> {
    ensure_supported()?;
    super::validate_version(version)?;
    let record = discovery::find(config, version)
        .ok_or_else(|| crate::tr!("error-studio-install-not-found", version = version))?;
    let request = prepare_launch_request(config, &record, project_mpr_path)?;
    let verified_executable = security::verify_and_lock_mendix_executable(&request.executable)?;
    let mut command = Command::new(verified_executable.path());
    if let Some(project) = request.project {
        command.arg(project);
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| crate::tr!("error-native-studio-launch", error = error))
}

fn prepare_launch_request(
    config: &AppConfig,
    record: &discovery::InstallationRecord,
    project_mpr_path: Option<&str>,
) -> Result<StudioLaunchRequest, String> {
    let executable = PathBuf::from(&record.studio.executable_path);
    if !executable.is_file() {
        return Err(crate::tr!(
            "error-script-studio-executable-not-found",
            path = executable.display()
        ));
    }
    let project = project_mpr_path
        .map(|path| validated_project_path(config, path))
        .transpose()?;
    Ok(StudioLaunchRequest {
        executable,
        project,
    })
}

pub(super) async fn install_studio<F>(
    config: &AppConfig,
    version: &str,
    installer_path: &Path,
    cancellation: CancellationToken,
    on_progress: F,
) -> Result<String, NativeOperationFailure>
where
    F: FnMut(StudioInstallProgress) + Send,
{
    ensure_supported()?;
    super::validate_version(version)?;
    ensure_not_cancelled(&cancellation)?;
    let discovery_config = config.clone();
    let discovery_version = version.to_string();
    let existing =
        tokio::task::spawn_blocking(move || discovery::find(&discovery_config, &discovery_version))
            .await
            .map_err(|error| crate::tr!("error-native-process-join", error = error))?;
    if let Some(record) = existing {
        let executable = PathBuf::from(record.studio.executable_path);
        let running = tokio::task::spawn_blocking(move || process::studio_is_running(&executable))
            .await
            .map_err(|error| crate::tr!("error-native-process-join", error = error))??;
        if running {
            return Err(crate::tr!("error-native-studio-running").into());
        }
    }
    ensure_not_cancelled(&cancellation)?;
    let installer = installer_path.to_path_buf();
    let verified = tokio::task::spawn_blocking(move || verified_execution_path(&installer))
        .await
        .map_err(|error| crate::tr!("error-native-process-join", error = error))??;
    let runner_cancellation = cancellation.clone();
    let installed_config = config.clone();
    let installed_version = version.to_string();
    install_lifecycle(
        version,
        installer_path,
        cancellation,
        on_progress,
        InstallLifecycleHooks {
            verify: move |_: &Path| Ok(verified),
            run_elevated: |executable: PathBuf, arguments: Vec<String>| async move {
                tokio::task::spawn_blocking(move || {
                    process::run_elevated(
                        &executable,
                        &arguments,
                        INSTALLER_PROCESS_POLICY,
                        &runner_cancellation,
                        "Studio Pro installer",
                    )
                })
                .await
                .map_err(|error| {
                    NativeOperationFailure::from(crate::tr!(
                        "error-native-process-join",
                        error = error
                    ))
                })?
                .map_err(NativeOperationFailure::from)
            },
            find_installed: move || {
                let config = installed_config.clone();
                let version = installed_version.clone();
                async move {
                    tokio::task::spawn_blocking(move || discovery::find(&config, &version))
                        .await
                        .ok()
                        .flatten()
                }
            },
            timing: INSTALL_TIMING,
        },
    )
    .await
}

fn verified_execution_path(path: &Path) -> Result<(PathBuf, security::VerifiedExecutable), String> {
    let verified = security::verify_and_lock_mendix_executable(path)?;
    Ok((verified.path().to_path_buf(), verified))
}

async fn install_lifecycle<F, V, G, R, RFuture, D, DFuture>(
    version: &str,
    installer_path: &Path,
    cancellation: CancellationToken,
    mut on_progress: F,
    mut hooks: InstallLifecycleHooks<V, R, D>,
) -> Result<String, NativeOperationFailure>
where
    F: FnMut(StudioInstallProgress),
    V: FnOnce(&Path) -> Result<(PathBuf, G), String>,
    R: FnOnce(PathBuf, Vec<String>) -> RFuture,
    RFuture: Future<Output = Result<u32, NativeOperationFailure>>,
    D: FnMut() -> DFuture,
    DFuture: Future<Output = Option<discovery::InstallationRecord>>,
{
    ensure_not_cancelled(&cancellation)?;
    on_progress(progress(StudioInstallPhase::Staging, 0.0, false));
    let (executable, verified_executable) =
        (hooks.verify)(installer_path).map_err(NativeOperationFailure::from)?;
    ensure_not_cancelled(&cancellation)?;
    on_progress(progress(StudioInstallPhase::Staging, 100.0, false));

    let arguments = vec![
        "/SP-".to_string(),
        "/SILENT".to_string(),
        "/SUPPRESSMSGBOXES".to_string(),
        "/NOCANCEL".to_string(),
        "/NORESTART".to_string(),
    ];
    on_progress(progress(StudioInstallPhase::Installing, 5.0, true));
    let exit_code = wait_for_installer(
        (hooks.run_elevated)(executable, arguments),
        INSTALLER_PROCESS_TIMEOUT,
        INSTALL_HEARTBEAT_INTERVAL,
        &mut on_progress,
    )
    .await?;
    drop(verified_executable);
    ensure_not_cancelled(&cancellation)?;
    if !SUCCESS_EXIT_CODES.contains(&exit_code) {
        return Err(NativeOperationFailure::from(crate::tr!(
            "error-script-installer-exit-code",
            code = exit_code
        )));
    }
    on_progress(progress(StudioInstallPhase::Finalizing, 100.0, false));
    ensure_not_cancelled(&cancellation)?;
    on_progress(progress(StudioInstallPhase::Verifying, 0.0, false));

    let started = tokio::time::Instant::now();
    loop {
        ensure_not_cancelled(&cancellation)?;
        if let Some(record) = (hooks.find_installed)().await {
            on_progress(progress(StudioInstallPhase::Verifying, 100.0, false));
            return Ok(record.studio.executable_path);
        }
        if started.elapsed() >= hooks.timing.timeout {
            break;
        }
        tokio::time::sleep(hooks.timing.poll_interval).await;
    }
    Err(NativeOperationFailure::from(crate::tr!(
        "error-script-studio-not-created",
        version = version
    )))
}

async fn wait_for_installer<F, P>(
    future: F,
    expected_runtime: Duration,
    heartbeat_interval: Duration,
    on_progress: &mut P,
) -> Result<u32, NativeOperationFailure>
where
    F: Future<Output = Result<u32, NativeOperationFailure>>,
    P: FnMut(StudioInstallProgress),
{
    let started = tokio::time::Instant::now();
    let heartbeat_interval = heartbeat_interval.max(Duration::from_millis(1));
    let first_heartbeat = tokio::time::Instant::now() + heartbeat_interval;
    let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result,
            _ = heartbeat.tick() => {
                let expected_seconds = expected_runtime.as_secs_f64().max(1.0);
                let elapsed_ratio = (started.elapsed().as_secs_f64() / expected_seconds).min(1.0);
                let percentage = (5.0 + elapsed_ratio * 85.0).min(90.0);
                on_progress(progress(StudioInstallPhase::Installing, percentage, true));
            }
        }
    }
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), NativeOperationFailure> {
    if cancellation.is_cancelled() {
        Err(NativeOperationFailure {
            message: "the Studio Pro installation was cancelled".to_string(),
            failure_kind: Some(CommandFailureKind::Cancelled),
        })
    } else {
        Ok(())
    }
}

pub(super) async fn uninstall_studio(
    config: &AppConfig,
    version: &str,
) -> Result<(), NativeOperationFailure> {
    ensure_supported()?;
    super::validate_version(version)?;
    let inspection_config = config.clone();
    let inspection_version = version.to_string();
    let (record, verified_uninstaller, running) = tokio::task::spawn_blocking(move || {
        let mut record =
            discovery::find(&inspection_config, &inspection_version).ok_or_else(|| {
                crate::tr!(
                    "error-studio-install-not-found",
                    version = inspection_version
                )
            })?;
        security::verify_mendix_executable(Path::new(&record.studio.executable_path))?;
        let verified_uninstaller =
            secure_uninstall_executable(&inspection_config, &mut record, || {
                process::system_executable("msiexec.exe")
            })?;
        let running = process::studio_is_running(Path::new(&record.studio.executable_path))?;
        Ok::<_, String>((record, verified_uninstaller, running))
    })
    .await
    .map_err(|error| crate::tr!("error-native-process-join", error = error))??;
    let cancellation = CancellationToken::default();
    let installed_config = config.clone();
    let installed_version = version.to_string();
    uninstall_lifecycle(
        record,
        move |_| Ok(running),
        move |executable, arguments| async move {
            let result = tokio::task::spawn_blocking(move || {
                process::run_elevated(
                    &executable,
                    &arguments,
                    UNINSTALLER_PROCESS_POLICY,
                    &cancellation,
                    "Studio Pro uninstaller",
                )
            })
            .await
            .map_err(|error| {
                NativeOperationFailure::from(crate::tr!("error-native-process-join", error = error))
            })?
            .map_err(NativeOperationFailure::from);
            drop(verified_uninstaller);
            result
        },
        move || {
            let config = installed_config.clone();
            let version = installed_version.clone();
            async move {
                tokio::task::spawn_blocking(move || discovery::find(&config, &version))
                    .await
                    .ok()
                    .flatten()
            }
        },
        UNINSTALL_TIMING,
    )
    .await
}

fn secure_uninstall_executable<F>(
    config: &AppConfig,
    record: &mut discovery::InstallationRecord,
    resolve_msiexec: F,
) -> Result<Option<security::VerifiedExecutable>, String>
where
    F: FnOnce() -> Result<PathBuf, String>,
{
    let version = record.studio.version.clone();
    let install_root = PathBuf::from(&record.studio.install_root);
    let uninstall = record
        .uninstall
        .as_mut()
        .ok_or_else(|| crate::tr!("error-native-uninstaller-metadata", version = version))?;
    let name = uninstall
        .executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("msiexec") || name.eq_ignore_ascii_case("msiexec.exe") {
        let executable = resolve_msiexec()?;
        if !executable.is_file()
            || (uninstall.executable.components().count() > 1
                && !same_windows_path(&uninstall.executable, &executable))
            || !valid_msiexec_uninstall_arguments(&uninstall.arguments)
        {
            return Err(crate::tr!(
                "error-native-uninstaller-metadata",
                version = version
            ));
        }
        uninstall.executable = executable;
        return Ok(None);
    }

    if uninstall.executable.components().count() == 1
        || !valid_mendix_uninstaller_path(config, &install_root, &uninstall.executable)
        || !valid_mendix_uninstaller_arguments(&uninstall.arguments)
    {
        return Err(crate::tr!(
            "error-native-uninstaller-metadata",
            version = version
        ));
    }
    let verified = security::verify_and_lock_mendix_executable(&uninstall.executable)?;
    uninstall.executable = verified.path().to_path_buf();
    Ok(Some(verified))
}

fn valid_msiexec_uninstall_arguments(arguments: &[String]) -> bool {
    let product_code = regex::Regex::new(
        r"(?i)^\{[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\}$",
    )
    .expect("MSI product code regex");
    let mut found_product = false;
    let mut expect_product = false;
    for argument in arguments {
        let value = argument.trim();
        if expect_product {
            if found_product || !product_code.is_match(value) {
                return false;
            }
            found_product = true;
            expect_product = false;
            continue;
        }
        if value.eq_ignore_ascii_case("/x") || value.eq_ignore_ascii_case("-x") {
            if found_product {
                return false;
            }
            expect_product = true;
            continue;
        }
        let inline_product = value.get(..2).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case("/x") || prefix.eq_ignore_ascii_case("-x")
        });
        if inline_product {
            let Some(code) = value.get(2..) else {
                return false;
            };
            if found_product || !product_code.is_match(code) {
                return false;
            }
            found_product = true;
            continue;
        }
        if !matches!(
            value.to_ascii_lowercase().as_str(),
            "/quiet" | "/qn" | "/passive" | "/norestart"
        ) {
            return false;
        }
    }
    found_product && !expect_product
}

fn valid_mendix_uninstaller_arguments(arguments: &[String]) -> bool {
    arguments.iter().all(|argument| {
        matches!(
            argument.trim().to_ascii_lowercase().as_str(),
            "/silent"
                | "/verysilent"
                | "/suppressmsgboxes"
                | "/norestart"
                | "/sp-"
                | "/currentuser"
                | "/allusers"
        )
    })
}

fn valid_mendix_uninstaller_path(
    config: &AppConfig,
    install_root: &Path,
    executable: &Path,
) -> bool {
    let file_name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !(file_name.starts_with("unins") || file_name.starts_with("uninstall"))
        || !executable.is_file()
    {
        return false;
    }

    let data_root = Path::new(&config.mendix_data_root);
    let expected_data_root = install_root
        .file_name()
        .map(|folder| data_root.join(folder));
    path_is_within(executable, install_root)
        || expected_data_root
            .as_deref()
            .is_some_and(|root| path_is_within(executable, root))
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = normalized_windows_path(path);
    let root = normalized_windows_path(root);
    path == root || path.starts_with(&format!("{root}\\"))
}

fn same_windows_path(left: &Path, right: &Path) -> bool {
    normalized_windows_path(left) == normalized_windows_path(right)
}

fn normalized_windows_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

async fn uninstall_lifecycle<I, R, RFuture, D, DFuture>(
    record: discovery::InstallationRecord,
    is_running: I,
    run_elevated: R,
    mut find_installed: D,
    timing: LifecycleTiming,
) -> Result<(), NativeOperationFailure>
where
    I: FnOnce(&Path) -> Result<bool, String>,
    R: FnOnce(PathBuf, Vec<String>) -> RFuture,
    RFuture: Future<Output = Result<u32, NativeOperationFailure>>,
    D: FnMut() -> DFuture,
    DFuture: Future<Output = Option<discovery::InstallationRecord>>,
{
    let executable_path = PathBuf::from(&record.studio.executable_path);
    if is_running(&executable_path).map_err(NativeOperationFailure::from)? {
        return Err(crate::tr!("error-native-studio-running").into());
    }
    let uninstall = record.uninstall.ok_or_else(|| {
        NativeOperationFailure::from(crate::tr!(
            "error-native-uninstaller-metadata",
            version = record.studio.version
        ))
    })?;
    let exit_code = run_elevated(uninstall.executable, uninstall.arguments).await?;
    if !SUCCESS_EXIT_CODES.contains(&exit_code) {
        return Err(NativeOperationFailure::from(crate::tr!(
            "error-script-uninstaller-exit-code",
            code = exit_code
        )));
    }

    let started = tokio::time::Instant::now();
    loop {
        if !executable_path.is_file() && find_installed().await.is_none() {
            return Ok(());
        }
        if started.elapsed() >= timing.timeout {
            break;
        }
        tokio::time::sleep(timing.poll_interval).await;
    }
    Err(NativeOperationFailure::from(crate::tr!(
        "error-script-uninstall-still-exists",
        path = executable_path.display()
    )))
}

pub(super) fn open_folder(path: &str) -> Result<(), String> {
    let directory = Path::new(path);
    if !directory.is_dir() {
        return Err(crate::tr!("error-directory-not-found", path = path));
    }
    Command::new("explorer.exe")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| crate::tr!("error-file-manager-open", error = error))
}

fn validated_project_path(config: &AppConfig, requested_path: &str) -> Result<PathBuf, String> {
    let requested = Path::new(requested_path);
    if !requested.is_file()
        || !requested
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mpr"))
    {
        return Err(crate::tr!("error-project-not-found"));
    }
    let workspace = Path::new(&config.shared_directory)
        .canonicalize()
        .map_err(|error| crate::tr!("error-shared-directory-inspect", error = error))?;
    let project = requested
        .canonicalize()
        .map_err(|_| crate::tr!("error-project-not-found"))?;
    if project.strip_prefix(&workspace).is_err() {
        return Err(crate::tr!("error-project-not-shared"));
    }
    Ok(project)
}

fn ensure_supported() -> Result<(), String> {
    if super::capabilities().supports_studio_management {
        Ok(())
    } else {
        Err(crate::tr!(
            "error-native-architecture",
            architecture = std::env::consts::ARCH
        ))
    }
}

fn progress(phase: StudioInstallPhase, percentage: f64, estimated: bool) -> StudioInstallProgress {
    StudioInstallProgress {
        phase,
        percentage: Some(percentage),
        estimated,
    }
}

#[cfg(test)]
mod tests {
    use super::discovery::{InstallationRecord, UninstallCommand};
    use super::{
        environment_status, install_lifecycle, prepare_launch_request, secure_uninstall_executable,
        uninstall_lifecycle, valid_mendix_uninstaller_arguments, valid_mendix_uninstaller_path,
        valid_msiexec_uninstall_arguments, validated_project_path, wait_for_installer,
        InstallLifecycleHooks, LifecycleTiming, NativeOperationFailure, StudioLaunchRequest,
    };
    use crate::models::{
        AppConfig, ContainerRuntime, HostPlatform, StudioInstallPhase, StudioVersion,
    };
    use crate::process::{CancellationToken, CommandFailureKind};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn config(root: &std::path::Path) -> AppConfig {
        AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: String::new(),
            compose_file: String::new(),
            container_runtime: ContainerRuntime::Docker,
            container_name: String::new(),
            api_url: String::new(),
            rdp_host: String::new(),
            rdp_port: 0,
            shared_directory: root.to_string_lossy().to_string(),
            windows_shared_directory: String::new(),
            freerdp_binary: String::new(),
            mendix_install_root: root.join("Mendix").to_string_lossy().to_string(),
            mendix_data_root: root.join("MendixData").to_string_lossy().to_string(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        }
    }

    fn unexpected_discovery(
        message: &'static str,
    ) -> std::future::Ready<Option<InstallationRecord>> {
        panic!("{message}")
    }

    #[test]
    fn native_environment_is_ready_without_winboat_when_workspace_exists() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let status = environment_status(&config(temporary.path()));
        assert_eq!(status.platform.kind, HostPlatform::WindowsNative);
        assert!(!status.platform.requires_winboat);
        assert!(status.ready);
        assert!(status.guest_online);
        assert!(!status.winboat_available);
    }

    #[test]
    fn accepts_only_mpr_files_inside_the_configured_workspace() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let project = temporary.path().join("Orders/Orders.mpr");
        fs::create_dir_all(project.parent().expect("project directory"))
            .expect("create project directory");
        fs::write(&project, b"mpr").expect("write project");
        assert_eq!(
            validated_project_path(&config, project.to_str().expect("utf8 path"))
                .expect("valid project"),
            project.canonicalize().expect("canonical project")
        );

        let outside = tempfile::NamedTempFile::with_suffix(".mpr").expect("outside project");
        assert!(
            validated_project_path(&config, outside.path().to_str().expect("utf8 path")).is_err()
        );
    }

    #[test]
    fn builds_an_exact_native_executable_and_mpr_launch_request() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let executable = temporary.path().join("11.12.2/modeler/StudioPro.exe");
        let project = temporary.path().join("Orders/Orders.mpr");
        fs::create_dir_all(executable.parent().expect("modeler directory"))
            .expect("create modeler directory");
        fs::create_dir_all(project.parent().expect("project directory"))
            .expect("create project directory");
        fs::write(&executable, b"Studio fixture").expect("write Studio fixture");
        fs::write(&project, b"MPR fixture").expect("write MPR fixture");
        let record = InstallationRecord {
            studio: StudioVersion {
                version: "11.12.2".into(),
                display_name: "Mendix 11.12.2".into(),
                executable_path: executable.to_string_lossy().to_string(),
                install_root: executable
                    .parent()
                    .and_then(|path| path.parent())
                    .expect("install root")
                    .to_string_lossy()
                    .to_string(),
                source: "Windows Registry".into(),
                removable: false,
            },
            uninstall: None,
        };

        let request = prepare_launch_request(
            &config,
            &record,
            Some(project.to_str().expect("UTF-8 project path")),
        )
        .expect("launch request");
        assert_eq!(
            request,
            StudioLaunchRequest {
                executable,
                project: Some(project.canonicalize().expect("canonical project")),
            }
        );
    }

    #[test]
    fn accepts_only_mendix_or_windows_installer_removal_executables() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let mut base_record = InstallationRecord {
            studio: StudioVersion {
                version: "11.12.2".into(),
                display_name: "Mendix 11.12.2".into(),
                executable_path: r"C:\Program Files\Mendix\11.12.2\modeler\StudioPro.exe".into(),
                install_root: r"C:\Program Files\Mendix\11.12.2".into(),
                source: "Windows Registry".into(),
                removable: true,
            },
            uninstall: Some(UninstallCommand {
                executable: PathBuf::from("MsiExec.exe"),
                arguments: vec![
                    "/X{3A591FB0-64D9-4C8E-8A84-026D7085291C}".into(),
                    "/quiet".into(),
                ],
            }),
        };
        let system_msiexec = temporary.path().join("System32/msiexec.exe");
        fs::create_dir_all(system_msiexec.parent().expect("System32 directory"))
            .expect("create System32 directory");
        fs::write(&system_msiexec, b"Windows Installer fixture")
            .expect("write Windows Installer fixture");
        assert!(secure_uninstall_executable(&config, &mut base_record, || {
            Ok(system_msiexec.clone())
        })
        .is_ok());
        assert_eq!(
            base_record
                .uninstall
                .as_ref()
                .expect("uninstall command")
                .executable,
            system_msiexec
        );

        let mut install_attempt = base_record.clone();
        install_attempt.uninstall = Some(UninstallCommand {
            executable: PathBuf::from("msiexec.exe"),
            arguments: vec!["/i".into(), r"C:\Users\Public\payload.msi".into()],
        });
        assert!(
            secure_uninstall_executable(&config, &mut install_attempt, || {
                Ok(system_msiexec.clone())
            })
            .is_err()
        );

        let mut arbitrary = InstallationRecord {
            uninstall: Some(UninstallCommand {
                executable: PathBuf::from("powershell.exe"),
                arguments: vec!["-Command".into(), "Remove-Item".into()],
            }),
            ..base_record
        };
        assert!(
            secure_uninstall_executable(&config, &mut arbitrary, || Ok(PathBuf::new())).is_err()
        );
    }

    #[test]
    fn constrains_uninstaller_paths_and_arguments_to_the_selected_installation() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let config = config(temporary.path());
        let install_root = temporary.path().join("Mendix/11.12.2");
        let expected = temporary
            .path()
            .join("MendixData/11.12.2/uninst/unins000.exe");
        let other_version = temporary
            .path()
            .join("MendixData/10.24.9/uninst/unins000.exe");
        fs::create_dir_all(expected.parent().expect("expected uninstaller parent"))
            .expect("expected uninstaller directory");
        fs::create_dir_all(
            other_version
                .parent()
                .expect("other version uninstaller parent"),
        )
        .expect("other version uninstaller directory");
        fs::write(&expected, b"fixture").expect("expected uninstaller");
        fs::write(&other_version, b"fixture").expect("other uninstaller");

        assert!(valid_mendix_uninstaller_path(
            &config,
            &install_root,
            &expected
        ));
        assert!(!valid_mendix_uninstaller_path(
            &config,
            &install_root,
            &other_version
        ));
        assert!(valid_mendix_uninstaller_arguments(&[
            "/SILENT".into(),
            "/NORESTART".into()
        ]));
        assert!(!valid_mendix_uninstaller_arguments(&[
            "/LOADINF=C:\\payload".into()
        ]));
        assert!(valid_msiexec_uninstall_arguments(&[
            "/x".into(),
            "{3A591FB0-64D9-4C8E-8A84-026D7085291C}".into(),
            "/qn".into()
        ]));
        assert!(!valid_msiexec_uninstall_arguments(&[
            "/x".into(),
            r"C:\Users\Public\payload.msi".into()
        ]));
    }

    #[tokio::test]
    async fn native_install_and_uninstall_lifecycle_runs_end_to_end_with_system_boundaries() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let executable = temporary
            .path()
            .join("Mendix/11.12.2/modeler/StudioPro.exe");
        let uninstaller = temporary
            .path()
            .join("MendixData/11.12.2/uninst/unins000.exe");
        fs::create_dir_all(executable.parent().expect("modeler directory"))
            .expect("create modeler directory");
        fs::create_dir_all(uninstaller.parent().expect("uninstaller directory"))
            .expect("create uninstaller directory");
        fs::write(&installer, b"signed installer fixture").expect("installer fixture");
        fs::write(&executable, b"studio fixture").expect("studio fixture");
        fs::write(&uninstaller, b"uninstaller fixture").expect("uninstaller fixture");

        let record = InstallationRecord {
            studio: StudioVersion {
                version: "11.12.2".into(),
                display_name: "Mendix 11.12.2".into(),
                executable_path: executable.to_string_lossy().to_string(),
                install_root: executable
                    .parent()
                    .and_then(|path| path.parent())
                    .expect("install root")
                    .to_string_lossy()
                    .to_string(),
                source: "Windows Registry".into(),
                removable: true,
            },
            uninstall: Some(UninstallCommand {
                executable: uninstaller.clone(),
                arguments: vec!["/SILENT".into()],
            }),
        };
        let timing = LifecycleTiming {
            timeout: Duration::from_millis(20),
            poll_interval: Duration::ZERO,
        };
        let installed = Arc::new(AtomicBool::new(false));
        let invocations = Arc::new(Mutex::new(Vec::<(PathBuf, Vec<String>)>::new()));
        let install_state = Arc::clone(&installed);
        let install_invocations = Arc::clone(&invocations);
        let find_state = Arc::clone(&installed);
        let install_record = record.clone();
        let verified_installer = temporary.path().join("canonical-installer.exe");
        let expected_installer = verified_installer.clone();
        let mut phases = Vec::new();

        let installed_path = install_lifecycle(
            "11.12.2",
            &installer,
            CancellationToken::default(),
            |progress| phases.push(progress.phase),
            InstallLifecycleHooks {
                verify: |path: &Path| {
                    assert_eq!(path, installer);
                    Ok((verified_installer, ()))
                },
                run_elevated: move |program: PathBuf, arguments: Vec<String>| async move {
                    install_invocations
                        .lock()
                        .expect("invocations")
                        .push((program, arguments));
                    install_state.store(true, Ordering::SeqCst);
                    Ok(0)
                },
                find_installed: move || {
                    std::future::ready(
                        find_state
                            .load(Ordering::SeqCst)
                            .then(|| install_record.clone()),
                    )
                },
                timing,
            },
        )
        .await
        .expect("native install lifecycle");

        assert_eq!(installed_path, executable.to_string_lossy());
        assert_eq!(
            phases,
            [
                StudioInstallPhase::Staging,
                StudioInstallPhase::Staging,
                StudioInstallPhase::Installing,
                StudioInstallPhase::Finalizing,
                StudioInstallPhase::Verifying,
                StudioInstallPhase::Verifying,
            ]
        );
        let uninstall_state = Arc::clone(&installed);
        let uninstall_invocations = Arc::clone(&invocations);
        let uninstall_executable = executable.clone();
        let find_state = Arc::clone(&installed);
        let uninstall_record = record.clone();

        uninstall_lifecycle(
            record,
            |path| {
                assert_eq!(path, executable);
                Ok(false)
            },
            move |program, arguments| async move {
                uninstall_invocations
                    .lock()
                    .expect("invocations")
                    .push((program, arguments));
                fs::remove_file(uninstall_executable).expect("remove Studio fixture");
                uninstall_state.store(false, Ordering::SeqCst);
                Ok(3010)
            },
            move || {
                std::future::ready(
                    find_state
                        .load(Ordering::SeqCst)
                        .then(|| uninstall_record.clone()),
                )
            },
            timing,
        )
        .await
        .expect("native uninstall lifecycle");

        let invocations = invocations.lock().expect("invocations");
        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].0, expected_installer);
        assert!(invocations[0].1.contains(&"/SILENT".to_string()));
        assert_eq!(invocations[1].0, uninstaller);
        assert_eq!(invocations[1].1, ["/SILENT"]);
        assert!(!installed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn native_install_stops_on_uac_cancellation_without_verifying_installation() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        fs::write(&installer, b"installer fixture").expect("installer fixture");
        let discovery_called = Arc::new(AtomicBool::new(false));
        let discovery_state = Arc::clone(&discovery_called);

        let error = install_lifecycle(
            "11.12.2",
            &installer,
            CancellationToken::default(),
            |_| {},
            InstallLifecycleHooks {
                verify: |path: &Path| Ok((path.to_path_buf(), ())),
                run_elevated: |_program: PathBuf, _arguments: Vec<String>| async {
                    Err("UAC cancelled".into())
                },
                find_installed: move || {
                    discovery_state.store(true, Ordering::SeqCst);
                    std::future::ready(None)
                },
                timing: LifecycleTiming {
                    timeout: Duration::ZERO,
                    poll_interval: Duration::ZERO,
                },
            },
        )
        .await
        .expect_err("UAC cancellation must fail");

        assert_eq!(error, "UAC cancelled");
        assert!(!discovery_called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn native_install_preserves_timeout_and_user_cancellation_kinds() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        fs::write(&installer, b"installer fixture").expect("installer fixture");

        let timeout = install_lifecycle(
            "11.12.2",
            &installer,
            CancellationToken::default(),
            |_| {},
            InstallLifecycleHooks {
                verify: |path: &Path| Ok((path.to_path_buf(), ())),
                run_elevated: |_program: PathBuf, _arguments: Vec<String>| async {
                    Err(NativeOperationFailure {
                        message: "fake elevated process timed out".into(),
                        failure_kind: Some(CommandFailureKind::Timeout),
                    })
                },
                find_installed: || unexpected_discovery("discovery must not run after timeout"),
                timing: LifecycleTiming {
                    timeout: Duration::ZERO,
                    poll_interval: Duration::ZERO,
                },
            },
        )
        .await
        .expect_err("timeout must fail");
        assert_eq!(timeout.failure_kind, Some(CommandFailureKind::Timeout));

        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let cancelled = install_lifecycle(
            "11.12.2",
            &installer,
            cancellation,
            |_| {},
            InstallLifecycleHooks {
                verify: |_: &Path| -> Result<(PathBuf, ()), String> {
                    panic!("cancelled lifecycle must not verify")
                },
                run_elevated: |_program: PathBuf, _arguments: Vec<String>| async { Ok(0) },
                find_installed: || unexpected_discovery("cancelled lifecycle must not discover"),
                timing: LifecycleTiming {
                    timeout: Duration::ZERO,
                    poll_interval: Duration::ZERO,
                },
            },
        )
        .await
        .expect_err("cancellation must fail");
        assert_eq!(cancelled.failure_kind, Some(CommandFailureKind::Cancelled));
    }

    #[tokio::test]
    async fn long_native_install_emits_estimated_heartbeats() {
        let mut progress = Vec::new();
        let exit_code = wait_for_installer(
            async {
                // Keep several heartbeats between the Windows timer quanta so this
                // assertion measures recurrence instead of scheduler resolution.
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(1641)
            },
            Duration::from_millis(500),
            Duration::from_millis(10),
            &mut |update| progress.push(update),
        )
        .await
        .expect("installer finishes with reboot success");

        assert_eq!(exit_code, 1641);
        assert!(progress.len() >= 2);
        assert!(progress
            .iter()
            .all(|update| { update.phase == StudioInstallPhase::Installing && update.estimated }));
        assert!(progress.windows(2).all(|pair| {
            pair[0].percentage.unwrap_or_default() <= pair[1].percentage.unwrap_or_default()
        }));
    }

    #[tokio::test]
    async fn native_install_rejects_signature_and_exit_code_failures_before_discovery() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let installer = temporary.path().join("Mendix-11.12.2-Setup.exe");
        fs::write(&installer, b"installer fixture").expect("installer fixture");
        let elevated = Arc::new(AtomicBool::new(false));
        let elevation_state = Arc::clone(&elevated);

        let signature_error = install_lifecycle(
            "11.12.2",
            &installer,
            CancellationToken::default(),
            |_| {},
            InstallLifecycleHooks {
                verify: |_: &Path| {
                    Err::<(PathBuf, ()), String>("invalid Authenticode signature".into())
                },
                run_elevated: move |_program: PathBuf, _arguments: Vec<String>| async move {
                    elevation_state.store(true, Ordering::SeqCst);
                    Ok(0)
                },
                find_installed: || {
                    unexpected_discovery("discovery must not run after signature failure")
                },
                timing: LifecycleTiming {
                    timeout: Duration::ZERO,
                    poll_interval: Duration::ZERO,
                },
            },
        )
        .await
        .expect_err("signature failure must stop installation");
        assert_eq!(signature_error, "invalid Authenticode signature");
        assert!(!elevated.load(Ordering::SeqCst));

        let discovery_called = Arc::new(AtomicBool::new(false));
        let discovery_state = Arc::clone(&discovery_called);
        let exit_error = install_lifecycle(
            "11.12.2",
            &installer,
            CancellationToken::default(),
            |_| {},
            InstallLifecycleHooks {
                verify: |path: &Path| Ok((path.to_path_buf(), ())),
                run_elevated: |_program: PathBuf, _arguments: Vec<String>| async { Ok(1603) },
                find_installed: move || {
                    discovery_state.store(true, Ordering::SeqCst);
                    std::future::ready(None)
                },
                timing: LifecycleTiming {
                    timeout: Duration::ZERO,
                    poll_interval: Duration::ZERO,
                },
            },
        )
        .await
        .expect_err("non-success installer exit must fail");
        assert!(!exit_error.is_empty());
        assert!(!discovery_called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn native_uninstall_refuses_running_or_unmanaged_installations() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let executable = temporary.path().join("11.12.2/modeler/StudioPro.exe");
        let uninstaller = temporary.path().join("unins000.exe");
        fs::create_dir_all(executable.parent().expect("modeler directory"))
            .expect("create modeler directory");
        fs::write(&executable, b"Studio fixture").expect("write Studio fixture");
        fs::write(&uninstaller, b"uninstaller fixture").expect("write uninstaller fixture");
        let record = InstallationRecord {
            studio: StudioVersion {
                version: "11.12.2".into(),
                display_name: "Mendix 11.12.2".into(),
                executable_path: executable.to_string_lossy().to_string(),
                install_root: temporary.path().to_string_lossy().to_string(),
                source: "Windows Registry".into(),
                removable: true,
            },
            uninstall: Some(UninstallCommand {
                executable: uninstaller,
                arguments: vec!["/SILENT".into()],
            }),
        };
        let elevated = Arc::new(AtomicBool::new(false));
        let elevation_state = Arc::clone(&elevated);
        let error = uninstall_lifecycle(
            record.clone(),
            |_| Ok(true),
            move |_program, _arguments| async move {
                elevation_state.store(true, Ordering::SeqCst);
                Ok(0)
            },
            || unexpected_discovery("discovery must not run while Studio Pro is running"),
            LifecycleTiming {
                timeout: Duration::ZERO,
                poll_interval: Duration::ZERO,
            },
        )
        .await
        .expect_err("running Studio Pro must block uninstall");
        assert!(!error.is_empty());
        assert!(!elevated.load(Ordering::SeqCst));

        let unmanaged = InstallationRecord {
            studio: StudioVersion {
                removable: false,
                ..record.studio
            },
            uninstall: None,
        };
        let error = uninstall_lifecycle(
            unmanaged,
            |_| Ok(false),
            |_program, _arguments| async { Ok(0) },
            || unexpected_discovery("discovery must not run without uninstall metadata"),
            LifecycleTiming {
                timeout: Duration::ZERO,
                poll_interval: Duration::ZERO,
            },
        )
        .await
        .expect_err("unmanaged portable install must not be removed");
        assert!(!error.is_empty());
    }
}
