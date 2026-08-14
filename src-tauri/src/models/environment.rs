use serde::{Deserialize, Serialize};

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

    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    pub const fn exists(self) -> bool {
        !matches!(self, Self::NotFound)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentStatus {
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
