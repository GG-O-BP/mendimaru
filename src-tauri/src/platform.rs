pub mod backend;
#[cfg(target_os = "windows")]
mod windows_native;

use crate::contracts::{
    ArtifactDescriptor, BackendId, BackendResult, BrowserTestRequest, BrowserTestSummary,
    CapabilityId, CapabilityManifest, CapabilitySnapshot, RuntimeBuildRequest, RuntimeBuildResult,
    RuntimeLogBatch, RuntimeStartRequest, RuntimeStatus, StudioSessionStatus,
};
use crate::models::{
    AppConfig, ContainerStatus, EnvironmentDiagnostic, EnvironmentDiagnosticAction,
    EnvironmentDiagnosticId, EnvironmentDiagnosticStatus, EnvironmentStatus, HostPlatform,
    PlatformCapabilities, StudioInstallProgress, StudioVersion,
};
use crate::process::CancellationToken;
use regex::Regex;
use std::path::Path;

pub fn capabilities() -> PlatformCapabilities {
    match capability_manifest(None) {
        Ok(manifest) => {
            let kind = match manifest.backend {
                BackendId::LinuxWinboat => HostPlatform::LinuxWinboat,
                BackendId::WindowsNative => HostPlatform::WindowsNative,
                BackendId::MacNative => HostPlatform::Unsupported,
            };
            let supports_studio_management = manifest.supports(CapabilityId::StudioDetect);
            let supports_installation = manifest.supports(CapabilityId::StudioInstall);
            let supports_uninstallation = manifest.supports(CapabilityId::StudioUninstall);
            PlatformCapabilities {
                kind,
                architecture: manifest.architecture,
                requires_winboat: manifest.backend == BackendId::LinuxWinboat,
                supports_studio_management,
                supports_installation,
                supports_uninstallation,
                supports_projects: !matches!(manifest.backend, BackendId::MacNative),
            }
        }
        Err(_) => PlatformCapabilities {
            kind: HostPlatform::Unsupported,
            architecture: std::env::consts::ARCH.to_string(),
            requires_winboat: false,
            supports_studio_management: false,
            supports_installation: false,
            supports_uninstallation: false,
            supports_projects: false,
        },
    }
}

pub fn capability_manifest(requested: Option<BackendId>) -> BackendResult<CapabilityManifest> {
    let selected = backend::select_backend_id(crate::contracts::PlatformId::current(), requested)?;
    Ok(backend::manifest_for(selected, std::env::consts::ARCH))
}

pub fn capability_snapshot(requested: Option<BackendId>) -> BackendResult<CapabilitySnapshot> {
    CapabilitySnapshot::capture(capability_manifest(requested)?)
}

pub const fn is_windows_native() -> bool {
    cfg!(target_os = "windows")
}

pub async fn environment_status(config: &AppConfig) -> EnvironmentStatus {
    #[cfg(target_os = "windows")]
    {
        return windows_native::environment_status(config);
    }
    #[cfg(target_os = "linux")]
    {
        return crate::winboat::environment_status(config).await;
    }
    #[allow(unreachable_code)]
    EnvironmentStatus {
        platform: capabilities(),
        ready: false,
        winboat_available: false,
        winboat_initialized: false,
        setup_pending: false,
        compose_available: false,
        runtime_available: false,
        freerdp_available: false,
        shared_directory_available: Path::new(&config.shared_directory).is_dir(),
        shared_mount_matches: false,
        container_status: ContainerStatus::NotFound,
        guest_online: false,
        diagnostics: vec![EnvironmentDiagnostic {
            id: EnvironmentDiagnosticId::SharedDirectory,
            status: if Path::new(&config.shared_directory).is_dir() {
                EnvironmentDiagnosticStatus::Success
            } else {
                EnvironmentDiagnosticStatus::Failure
            },
            observed: None,
            action: (!Path::new(&config.shared_directory).is_dir())
                .then_some(EnvironmentDiagnosticAction::OpenSettings),
            error_code: None,
        }],
    }
}

pub async fn installed_versions(config: &AppConfig) -> BackendResult<Vec<StudioVersion>> {
    backend::active_backend(config, None)?.detect().await
}

pub async fn studio_sessions(config: &AppConfig) -> BackendResult<Vec<StudioSessionStatus>> {
    backend::active_backend(config, None)?.sessions().await
}

pub async fn studio_session_status(
    config: &AppConfig,
    session_id: &str,
) -> BackendResult<StudioSessionStatus> {
    validate_studio_session_id(session_id)
        .map_err(crate::contracts::BackendError::invalid_request)?;
    backend::StudioBackend::status(backend::active_backend(config, None)?.as_ref(), session_id)
        .await
}

pub async fn reconnect_studio_session(config: &AppConfig, session_id: &str) -> BackendResult<()> {
    validate_studio_session_id(session_id)
        .map_err(crate::contracts::BackendError::invalid_request)?;
    backend::active_backend(config, None)?
        .reconnect(session_id)
        .await
}

pub async fn stop_studio_session(config: &AppConfig, session_id: &str) -> BackendResult<()> {
    validate_studio_session_id(session_id)
        .map_err(crate::contracts::BackendError::invalid_request)?;
    backend::StudioBackend::stop(backend::active_backend(config, None)?.as_ref(), session_id).await
}

pub async fn build_runtime(
    config: &AppConfig,
    request: &RuntimeBuildRequest,
) -> BackendResult<RuntimeBuildResult> {
    backend::active_backend(config, None)?.build(request).await
}

pub async fn start_runtime(
    config: &AppConfig,
    request: &RuntimeStartRequest,
) -> BackendResult<RuntimeStatus> {
    backend::RuntimeBackend::start(backend::active_backend(config, None)?.as_ref(), request).await
}

pub async fn wait_runtime(config: &AppConfig, session_id: &str) -> BackendResult<RuntimeStatus> {
    backend::RuntimeBackend::wait(backend::active_backend(config, None)?.as_ref(), session_id).await
}

pub async fn runtime_status(config: &AppConfig, session_id: &str) -> BackendResult<RuntimeStatus> {
    backend::RuntimeBackend::status(backend::active_backend(config, None)?.as_ref(), session_id)
        .await
}

pub async fn runtime_url(config: &AppConfig, session_id: &str) -> BackendResult<String> {
    backend::active_backend(config, None)?.url(session_id).await
}

pub async fn stop_runtime(config: &AppConfig, session_id: &str) -> BackendResult<()> {
    crate::platform::backend::RuntimeBackend::stop(
        backend::active_backend(config, None)?.as_ref(),
        session_id,
    )
    .await
}

pub async fn runtime_logs(
    config: &AppConfig,
    session_id: &str,
    cursor: Option<&str>,
) -> BackendResult<RuntimeLogBatch> {
    backend::active_backend(config, None)?
        .logs(session_id, cursor)
        .await
}

pub async fn run_browser_test(
    config: &AppConfig,
    request: &BrowserTestRequest,
) -> BackendResult<BrowserTestSummary> {
    backend::active_backend(config, None)?.test(request).await
}

pub async fn browser_artifacts(
    config: &AppConfig,
    session_id: &str,
) -> BackendResult<Vec<ArtifactDescriptor>> {
    backend::active_backend(config, None)?
        .artifacts(session_id)
        .await
}

fn validate_studio_session_id(session_id: &str) -> Result<(), String> {
    const WINDOWS_EPOCH_TICKS: u64 = 621_355_968_000_000_000;
    if session_id.len() > 48 {
        return Err("the Studio Pro session identifier is invalid".to_string());
    }
    let value = session_id
        .strip_prefix("studio-")
        .ok_or_else(|| "the Studio Pro session identifier is invalid".to_string())?;
    let (process_id, started_ticks) = value
        .split_once('-')
        .ok_or_else(|| "the Studio Pro session identifier is invalid".to_string())?;
    if process_id
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .is_none()
        || started_ticks
            .parse::<u64>()
            .ok()
            .filter(|value| *value > WINDOWS_EPOCH_TICKS)
            .is_none()
    {
        return Err("the Studio Pro session identifier is invalid".to_string());
    }
    Ok(())
}

pub async fn launch_studio(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
    project_mpr_path: Option<&str>,
) -> BackendResult<()> {
    let selected = backend::active_backend(config, None)?;
    backend::StudioBackend::start(selected.as_ref(), version, operation_id, project_mpr_path).await
}

pub async fn uninstall_studio(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
) -> BackendResult<()> {
    backend::active_backend(config, None)?
        .uninstall(version, operation_id)
        .await
}

pub async fn install_studio<F>(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
    installer_path: &Path,
    expected_sha256: &str,
    cancellation: CancellationToken,
    on_progress: F,
) -> BackendResult<String>
where
    F: FnMut(StudioInstallProgress) + Send,
{
    let mut on_progress = on_progress;
    backend::active_backend(config, None)?
        .install(
            version,
            operation_id,
            installer_path,
            expected_sha256,
            cancellation,
            &mut on_progress,
        )
        .await
}

#[cfg(target_os = "windows")]
pub fn verify_native_installer(path: &Path) -> Result<String, String> {
    windows_native::verify_installer(path)
}

pub fn open_folder(path: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return windows_native::open_folder(path);
    }
    #[cfg(target_os = "linux")]
    {
        return crate::winboat::open_linux_folder(path);
    }
    #[allow(unreachable_code)]
    Err(crate::tr!("error-platform-unsupported"))
}

pub fn validate_version(version: &str) -> Result<(), String> {
    let pattern = Regex::new(r"(?i)^\d+\.\d+\.\d+(?:\.\d+)?(?:-(?:beta|rc)(?:\.?\d+)?)?$")
        .map_err(|error| error.to_string())?;
    if pattern.is_match(version) {
        Ok(())
    } else {
        Err(crate::tr!("error-version-format"))
    }
}

#[cfg(test)]
mod tests {
    use super::{capabilities, validate_studio_session_id, validate_version};
    use crate::models::HostPlatform;

    #[test]
    fn current_platform_exposes_a_single_coherent_capability_set() {
        let capabilities = capabilities();
        if cfg!(target_os = "windows") {
            assert_eq!(capabilities.kind, HostPlatform::WindowsNative);
            assert!(!capabilities.requires_winboat);
            assert!(capabilities.supports_projects);
        } else if cfg!(target_os = "linux") {
            assert_eq!(capabilities.kind, HostPlatform::LinuxWinboat);
            assert!(capabilities.requires_winboat);
        } else {
            assert_eq!(capabilities.kind, HostPlatform::Unsupported);
        }
    }

    #[test]
    fn accepts_release_build_and_semver_prerelease_versions_only() {
        assert!(validate_version("11.12.2").is_ok());
        assert!(validate_version("10.24.0.12345").is_ok());
        assert!(validate_version("11.6.0-beta.1").is_ok());
        assert!(validate_version("11.0.0-rc1").is_ok());
        assert!(validate_version("11.12.2; calc.exe").is_err());
        assert!(validate_version("../11.12.2").is_err());
    }

    #[test]
    fn accepts_only_bounded_pid_and_start_time_session_identifiers() {
        assert!(validate_studio_session_id("studio-4242-638908236000000000").is_ok());
        assert!(validate_studio_session_id("studio-0-638908236000000000").is_err());
        assert!(validate_studio_session_id("studio-4242-1").is_err());
        assert!(validate_studio_session_id("studio-4242-638908236000000000-extra").is_err());
        assert!(validate_studio_session_id(&format!("studio-{}", "9".repeat(100))).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "installs, launches, observes, stops, and uninstalls a disposable Studio Pro version"]
    fn live_e2e_linux_winboat_backend_lifecycle() {
        use crate::app_paths::AppPaths;
        use crate::contracts::{
            BackendErrorCode, BackendId, CapabilityId, PlatformId, StudioConnectionState,
            StudioProcessState,
        };
        use crate::models::AppConfig;
        use crate::models::StudioInstallPhase;
        use sha2::{Digest, Sha256};
        use std::fs::File;
        use std::io::Read;
        use std::path::Path;
        use std::time::{SystemTime, UNIX_EPOCH};

        struct Cleanup<'a> {
            config: &'a AppConfig,
            version: &'a str,
        }

        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let sessions =
                    tauri::async_runtime::block_on(crate::platform::studio_sessions(self.config));
                if let Ok(sessions) = sessions {
                    for session in sessions
                        .into_iter()
                        .filter(|session| session.version == self.version)
                    {
                        if let Err(error) = tauri::async_runtime::block_on(
                            crate::platform::stop_studio_session(self.config, &session.session_id),
                        ) {
                            eprintln!(
                                "lifecycle cleanup could not stop {}: {error}",
                                session.session_id
                            );
                        }
                    }
                }
                let installed = tauri::async_runtime::block_on(
                    crate::platform::installed_versions(self.config),
                );
                if installed.is_ok_and(|installed| {
                    installed.iter().any(|item| item.version == self.version)
                }) {
                    let nonce = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|duration| duration.as_nanos())
                        .unwrap_or_default();
                    if let Err(error) =
                        tauri::async_runtime::block_on(crate::platform::uninstall_studio(
                            self.config,
                            self.version,
                            &format!("cleanup-{}-{nonce}", self.version),
                        ))
                    {
                        eprintln!(
                            "lifecycle cleanup could not uninstall {}: {error}",
                            self.version
                        );
                    }
                }
            }
        }

        fn phase_rank(phase: StudioInstallPhase) -> u8 {
            match phase {
                StudioInstallPhase::Staging => 0,
                StudioInstallPhase::Installing => 1,
                StudioInstallPhase::Finalizing => 2,
                StudioInstallPhase::Verifying => 3,
            }
        }

        fn control_artifacts(config: &AppConfig) -> Vec<String> {
            let directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
                Err(error) => panic!("inspect operation control artifacts: {error}"),
            };
            let mut artifacts = entries
                .map(|entry| entry.expect("read operation artifact entry"))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| {
                    name.ends_with(".control.json") || name.ends_with(".control.json.tmp")
                })
                .collect::<Vec<_>>();
            artifacts.sort();
            artifacts
        }

        fn host_staging_artifacts(config: &AppConfig) -> Vec<String> {
            let directory = Path::new(&config.shared_directory).join(".mendimaru/installers");
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
                Err(error) => panic!("inspect host installer staging artifacts: {error}"),
            };
            let mut artifacts = entries
                .map(|entry| entry.expect("read host staging artifact entry"))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("mendimaru-installer-stage-"))
                .collect::<Vec<_>>();
            artifacts.sort();
            artifacts
        }

        assert_eq!(
            std::env::var("MENDIMARU_E2E_ALLOW_MUTATION").as_deref(),
            Ok("1"),
            "set MENDIMARU_E2E_ALLOW_MUTATION=1 to mutate the live WinBoat VM"
        );
        let version = std::env::var("MENDIMARU_E2E_VERSION")
            .expect("set MENDIMARU_E2E_VERSION to the exact test version");
        validate_version(&version).expect("the E2E version must be valid");
        crate::i18n::initialize("en-US").expect("English localization initializes");
        let config =
            crate::config::detect_config().expect("the live WinBoat configuration must exist");

        let manifest = super::capability_manifest(None).expect("capabilities must resolve");
        assert_eq!(manifest.backend, BackendId::LinuxWinboat);
        assert_eq!(manifest.host_platform, PlatformId::Linux);
        assert_eq!(manifest.studio_platform, PlatformId::Windows);
        for capability in [
            CapabilityId::StudioDetect,
            CapabilityId::StudioInstall,
            CapabilityId::StudioUninstall,
            CapabilityId::StudioStart,
            CapabilityId::StudioStatus,
            CapabilityId::StudioStop,
        ] {
            assert!(manifest.supports(capability), "missing {capability}");
        }

        let environment = tauri::async_runtime::block_on(super::environment_status(&config));
        assert!(environment.guest_online, "the WinBoat guest must be online");
        let baseline_control_artifacts = control_artifacts(&config);
        let baseline_staging_artifacts = host_staging_artifacts(&config);

        let baseline = tauri::async_runtime::block_on(super::installed_versions(&config))
            .expect("the adapter must detect installed versions");
        assert!(
            baseline.iter().all(|item| item.version != version),
            "refusing to delete a preinstalled {version}; select an absent disposable version"
        );
        let baseline_sessions = tauri::async_runtime::block_on(super::studio_sessions(&config))
            .expect("the adapter must inspect the initial Studio sessions");
        assert!(
            baseline_sessions
                .iter()
                .all(|session| session.version != version),
            "the disposable version already has a running session"
        );
        let _cleanup = Cleanup {
            config: &config,
            version: &version,
        };

        let installer_path = AppPaths::discover_for_cli()
            .expect("resolve the host-private app cache")
            .cache_directory()
            .join("installers")
            .join(format!("Mendix-{version}-Setup.exe"));
        assert!(
            installer_path.is_file(),
            "private cached installer does not exist: {}",
            installer_path.display()
        );
        let mut installer = File::open(&installer_path).expect("open the E2E installer");
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let count = installer.read(&mut buffer).expect("hash the E2E installer");
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let expected_sha256 = format!("{:x}", hasher.finalize());
        let installer_size = installer.metadata().expect("inspect E2E installer").len();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must follow the Unix epoch")
            .as_nanos();

        let missing_launch = tauri::async_runtime::block_on(super::launch_studio(
            &config,
            &version,
            &format!("missing-launch-{version}-{nonce}"),
            None,
        ))
        .expect_err("an absent Studio version must not launch");
        assert_eq!(missing_launch.capability, Some(CapabilityId::StudioStart));

        let mut progress = Vec::new();
        let executable = tauri::async_runtime::block_on(super::install_studio(
            &config,
            &version,
            &format!("install-{version}-{nonce}"),
            &installer_path,
            &expected_sha256,
            crate::process::CancellationToken::default(),
            |update| {
                eprintln!(
                    "lifecycle install: phase={:?} percentage={:?} estimated={}",
                    update.phase, update.percentage, update.estimated
                );
                progress.push(update);
            },
        ))
        .expect("the adapter must install Studio Pro");
        assert!(executable.to_ascii_lowercase().ends_with("studiopro.exe"));
        assert_eq!(
            progress.first().map(|update| update.phase),
            Some(StudioInstallPhase::Staging)
        );
        for phase in [
            StudioInstallPhase::Staging,
            StudioInstallPhase::Installing,
            StudioInstallPhase::Finalizing,
            StudioInstallPhase::Verifying,
        ] {
            assert!(
                progress.iter().any(|update| update.phase == phase),
                "the adapter did not report {phase:?}"
            );
        }
        assert!(progress.iter().all(|update| update
            .percentage
            .is_none_or(|percentage| (0.0..=100.0).contains(&percentage))));
        assert!(progress
            .windows(2)
            .all(|updates| phase_rank(updates[0].phase) <= phase_rank(updates[1].phase)));
        assert_eq!(
            progress
                .last()
                .map(|update| (update.phase, update.percentage, update.estimated)),
            Some((StudioInstallPhase::Verifying, Some(100.0), false))
        );

        let installed = tauri::async_runtime::block_on(super::installed_versions(&config))
            .expect("the adapter must detect the new installation");
        let exact_installations = installed
            .iter()
            .filter(|item| item.version == version)
            .collect::<Vec<_>>();
        assert_eq!(exact_installations.len(), 1);
        assert!(exact_installations[0]
            .executable_path
            .eq_ignore_ascii_case(&executable));

        tauri::async_runtime::block_on(super::launch_studio(
            &config,
            &version,
            &format!("launch-{version}-{nonce}"),
            None,
        ))
        .expect("the adapter must launch the exact Studio Pro version");

        let sessions = tauri::async_runtime::block_on(super::studio_sessions(&config))
            .expect("the running Studio session must be observable");
        let launched = sessions
            .iter()
            .filter(|session| session.version == version)
            .collect::<Vec<_>>();
        assert_eq!(launched.len(), 1, "exactly one test session must run");
        let launched = launched[0];
        assert_eq!(launched.state, StudioProcessState::Running);
        assert_eq!(launched.connection, StudioConnectionState::Connected);
        assert!(launched.process_id.is_some());
        assert!(launched.started_at.is_some());
        let session_id = launched.session_id.clone();
        let status =
            tauri::async_runtime::block_on(super::studio_session_status(&config, &session_id))
                .expect("the exact launched process identity must remain queryable");
        assert_eq!(status.session_id, session_id);

        let running_uninstall = tauri::async_runtime::block_on(super::uninstall_studio(
            &config,
            &version,
            &format!("running-uninstall-{version}-{nonce}"),
        ))
        .expect_err("uninstall must refuse a running exact-version process");
        assert_eq!(
            running_uninstall.capability,
            Some(CapabilityId::StudioUninstall)
        );
        assert_eq!(running_uninstall.code, BackendErrorCode::OperationFailed);
        assert!(
            tauri::async_runtime::block_on(super::installed_versions(&config))
                .expect("installation must remain after rejected uninstall")
                .iter()
                .any(|item| item.version == version)
        );

        tauri::async_runtime::block_on(super::stop_studio_session(&config, &session_id))
            .expect("the exact Studio process must close normally");
        let ended_status =
            tauri::async_runtime::block_on(super::studio_session_status(&config, &session_id))
                .expect_err("the closed process identity must no longer resolve");
        assert_eq!(ended_status.capability, Some(CapabilityId::StudioStatus));
        let repeated_stop =
            tauri::async_runtime::block_on(super::stop_studio_session(&config, &session_id))
                .expect_err("a stale process identity must not close another process");
        assert_eq!(repeated_stop.capability, Some(CapabilityId::StudioStop));
        assert!(
            tauri::async_runtime::block_on(super::studio_sessions(&config))
                .expect("sessions must be readable after close")
                .iter()
                .all(|session| session.version != version)
        );

        tauri::async_runtime::block_on(super::uninstall_studio(
            &config,
            &version,
            &format!("uninstall-{version}-{nonce}"),
        ))
        .expect("the adapter must uninstall the exact Studio Pro version");
        let final_installed = tauri::async_runtime::block_on(super::installed_versions(&config))
            .expect("the adapter must detect versions after uninstall");
        assert!(final_installed.iter().all(|item| item.version != version));

        let missing_uninstall = tauri::async_runtime::block_on(super::uninstall_studio(
            &config,
            &version,
            &format!("missing-uninstall-{version}-{nonce}"),
        ))
        .expect_err("a second uninstall must not report false success");
        assert_eq!(
            missing_uninstall.capability,
            Some(CapabilityId::StudioUninstall)
        );
        let missing_launch = tauri::async_runtime::block_on(super::launch_studio(
            &config,
            &version,
            &format!("post-delete-launch-{version}-{nonce}"),
            None,
        ))
        .expect_err("a deleted Studio version must not launch");
        assert_eq!(missing_launch.capability, Some(CapabilityId::StudioStart));

        let mut before = baseline
            .iter()
            .map(|item| (&item.version, &item.executable_path))
            .collect::<Vec<_>>();
        before.sort();
        let mut after = final_installed
            .iter()
            .map(|item| (&item.version, &item.executable_path))
            .collect::<Vec<_>>();
        after.sort();
        assert_eq!(
            after, before,
            "the test modified another Studio installation"
        );
        assert_eq!(
            File::open(&installer_path)
                .expect("the cached installer must remain")
                .metadata()
                .expect("inspect cached installer after lifecycle")
                .len(),
            installer_size
        );
        assert_eq!(
            control_artifacts(&config),
            baseline_control_artifacts,
            "the lifecycle leaked an authenticated control artifact"
        );
        assert_eq!(
            host_staging_artifacts(&config),
            baseline_staging_artifacts,
            "the lifecycle leaked a shared host installer staging file"
        );
    }
}
