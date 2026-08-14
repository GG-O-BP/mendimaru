#[cfg(target_os = "windows")]
mod windows_native;

use crate::models::{
    AppConfig, ContainerStatus, EnvironmentStatus, HostPlatform, PlatformCapabilities,
    StudioInstallProgress, StudioVersion,
};
use regex::Regex;
use std::path::Path;

pub fn capabilities() -> PlatformCapabilities {
    let architecture = std::env::consts::ARCH.to_string();
    #[cfg(target_os = "windows")]
    {
        let supported = matches!(std::env::consts::ARCH, "x86_64" | "aarch64");
        return PlatformCapabilities {
            kind: HostPlatform::WindowsNative,
            architecture,
            requires_winboat: false,
            supports_studio_management: supported,
            supports_installation: supported,
            supports_uninstallation: supported,
            supports_projects: true,
        };
    }
    #[cfg(target_os = "linux")]
    {
        return PlatformCapabilities {
            kind: HostPlatform::LinuxWinboat,
            architecture,
            requires_winboat: true,
            supports_studio_management: true,
            supports_installation: true,
            supports_uninstallation: true,
            supports_projects: true,
        };
    }
    #[allow(unreachable_code)]
    PlatformCapabilities {
        kind: HostPlatform::Unsupported,
        architecture,
        requires_winboat: false,
        supports_studio_management: false,
        supports_installation: false,
        supports_uninstallation: false,
        supports_projects: false,
    }
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
    }
}

pub async fn installed_versions(config: &AppConfig) -> Result<Vec<StudioVersion>, String> {
    #[cfg(target_os = "windows")]
    {
        return windows_native::installed_versions(config);
    }
    #[cfg(target_os = "linux")]
    {
        return crate::winboat::installed_versions(config).await;
    }
    #[allow(unreachable_code)]
    Err(crate::tr!("error-platform-unsupported"))
}

pub async fn launch_studio(
    config: &AppConfig,
    version: &str,
    project_mpr_path: Option<&str>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return windows_native::launch_studio(config, version, project_mpr_path);
    }
    #[cfg(target_os = "linux")]
    {
        return crate::winboat::launch_studio(config, version, project_mpr_path).await;
    }
    #[allow(unreachable_code)]
    Err(crate::tr!("error-platform-unsupported"))
}

pub async fn uninstall_studio(config: &AppConfig, version: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        return windows_native::uninstall_studio(config, version).await;
    }
    #[cfg(target_os = "linux")]
    {
        return crate::winboat::launch_uninstaller(config, version).await;
    }
    #[allow(unreachable_code)]
    Err(crate::tr!("error-platform-unsupported"))
}

pub async fn install_studio<F>(
    config: &AppConfig,
    version: &str,
    installer_path: &Path,
    expected_sha256: &str,
    on_progress: F,
) -> Result<String, String>
where
    F: FnMut(StudioInstallProgress) + Send,
{
    #[cfg(target_os = "windows")]
    {
        let _ = expected_sha256;
        return windows_native::install_studio(config, version, installer_path, on_progress).await;
    }
    #[cfg(target_os = "linux")]
    {
        let windows_installer_path = crate::projects::linux_path_to_windows_share(
            Path::new(&config.shared_directory),
            installer_path,
            &config.windows_shared_directory,
        )?;
        return crate::winboat::install_studio(
            config,
            version,
            &windows_installer_path,
            expected_sha256,
            on_progress,
        )
        .await;
    }
    #[allow(unreachable_code)]
    {
        let _ = (
            config,
            version,
            installer_path,
            expected_sha256,
            on_progress,
        );
        Err(crate::tr!("error-platform-unsupported"))
    }
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
    use super::{capabilities, validate_version};
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
}
