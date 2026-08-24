use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostPlatform {
    LinuxWinboat,
    WindowsNative,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub kind: HostPlatform,
    pub architecture: String,
    pub requires_winboat: bool,
    pub supports_studio_management: bool,
    pub supports_installation: bool,
    pub supports_uninstallation: bool,
    pub supports_projects: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerStatus {
    Created,
    Restarting,
    Running,
    Removing,
    Paused,
    Exited,
    Dead,
    NotFound,
    Unknown,
}

impl ContainerStatus {
    #[cfg(any(target_os = "linux", test))]
    pub fn from_runtime(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "restarting" => Self::Restarting,
            "running" => Self::Running,
            "removing" => Self::Removing,
            "paused" => Self::Paused,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            "not-found" => Self::NotFound,
            _ => Self::Unknown,
        }
    }

    #[cfg(target_os = "linux")]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    #[cfg(target_os = "linux")]
    pub const fn exists(self) -> bool {
        !matches!(self, Self::NotFound)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentStatus {
    pub platform: PlatformCapabilities,
    pub ready: bool,
    pub winboat_available: bool,
    pub winboat_initialized: bool,
    pub setup_pending: bool,
    pub compose_available: bool,
    pub runtime_available: bool,
    pub freerdp_available: bool,
    pub shared_directory_available: bool,
    pub shared_mount_matches: bool,
    pub container_status: ContainerStatus,
    pub guest_online: bool,
}
