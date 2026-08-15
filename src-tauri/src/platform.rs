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
    #[ignore = "installs, launches, and uninstalls Studio Pro through the live backend contract"]
    fn live_e2e_linux_winboat_backend_lifecycle() {
        use crate::contracts::{BackendId, CapabilityId, PlatformId};
        use crate::models::StudioInstallPhase;
        use sha2::{Digest, Sha256};
        use std::fs::File;
        use std::io::Read;
        use std::path::Path;

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
        assert!(manifest.supports(CapabilityId::StudioDetect));
        assert!(manifest.supports(CapabilityId::StudioInstall));
        assert!(!manifest.supports(CapabilityId::RuntimeStart));

        let environment = tauri::async_runtime::block_on(super::environment_status(&config));
        assert!(environment.guest_online, "the WinBoat guest must be online");

        let installed = tauri::async_runtime::block_on(super::installed_versions(&config))
            .expect("the adapter must detect installed versions");
        if installed.iter().any(|item| item.version == version) {
            tauri::async_runtime::block_on(super::uninstall_studio(
                &config,
                &version,
                &format!("uninstall-{version}-preflight"),
            ))
            .expect("the adapter must normalize a preinstalled E2E version");
        }

        let installer_path = Path::new(&config.shared_directory)
            .join(".mendimaru/installers")
            .join(format!("Mendix-{version}-Setup.exe"));
        assert!(
            installer_path.is_file(),
            "cached installer does not exist: {}",
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
        let mut progress = Vec::new();
        let executable = tauri::async_runtime::block_on(super::install_studio(
            &config,
            &version,
            &format!("install-{version}-lifecycle"),
            &installer_path,
            &expected_sha256,
            |update| progress.push(update),
        ))
        .expect("the adapter must install Studio Pro");
        assert!(executable.to_ascii_lowercase().ends_with("studiopro.exe"));
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

        let installed = tauri::async_runtime::block_on(super::installed_versions(&config))
            .expect("the adapter must detect the new installation");
        assert!(installed.iter().any(|item| {
            item.version == version && item.executable_path.eq_ignore_ascii_case(&executable)
        }));

        tauri::async_runtime::block_on(super::launch_studio(
            &config,
            &version,
            &format!("launch-{version}-lifecycle"),
            None,
        ))
        .expect("the adapter must launch the exact Studio Pro version");
        tauri::async_runtime::block_on(super::uninstall_studio(
            &config,
            &version,
            &format!("uninstall-{version}-lifecycle"),
        ))
        .expect("the adapter must uninstall the exact Studio Pro version");
        let installed = tauri::async_runtime::block_on(super::installed_versions(&config))
            .expect("the adapter must detect versions after uninstall");
        assert!(installed.iter().all(|item| item.version != version));
    }
}
