use serde::{Deserialize, Serialize};
use std::fmt;

fn default_language_preference() -> String {
    "system".to_string()
}

fn default_windows_studio_paths() -> Vec<String> {
    Vec::new()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerRuntime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

impl fmt::Display for ContainerRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default = "default_language_preference")]
    pub language_preference: String,
    #[serde(default)]
    pub winboat_setup_pending: bool,
    pub winboat_executable: String,
    pub compose_file: String,
    pub container_runtime: ContainerRuntime,
    pub container_name: String,
    pub api_url: String,
    pub rdp_host: String,
    pub rdp_port: u16,
    pub shared_directory: String,
    pub windows_shared_directory: String,
    pub freerdp_binary: String,
    pub mendix_install_root: String,
    pub mendix_data_root: String,
    #[serde(default = "default_windows_studio_paths")]
    pub windows_studio_paths: Vec<String>,
    pub startup_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSaveResult {
    pub config: AppConfig,
    pub mount_changed: bool,
    pub container_recreated: bool,
}
