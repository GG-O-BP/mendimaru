use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn default_language_preference() -> String {
    "system".to_string()
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
    pub container_runtime: String,
    pub container_name: String,
    pub api_url: String,
    pub rdp_host: String,
    pub rdp_port: u16,
    pub shared_directory: String,
    pub windows_shared_directory: String,
    pub freerdp_binary: String,
    pub mendix_install_root: String,
    pub mendix_data_root: String,
    pub startup_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSaveResult {
    pub config: AppConfig,
    pub mount_changed: bool,
    pub container_recreated: bool,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WinApp {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StudioVersion {
    pub version: String,
    pub display_name: String,
    pub executable_path: String,
    pub install_root: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MendixProject {
    pub name: String,
    pub directory: String,
    pub mpr_path: String,
    pub windows_path: String,
    pub version: Option<String>,
    pub last_modified: Option<String>,
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
    pub container_status: String,
    pub guest_online: bool,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadableVersion {
    pub version: String,
    pub release_date: Option<String>,
    pub release_notes_url: Option<String>,
    pub is_lts: bool,
    pub is_beta: bool,
    pub is_mts: bool,
    pub is_latest: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioVersionCatalog {
    pub versions: Vec<DownloadableVersion>,
    pub loaded_pages: Vec<u32>,
    pub total_count: Option<u32>,
    pub fetched_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub version: String,
    pub installer_path: String,
    pub windows_installer_path: String,
    pub downloaded: bool,
    pub installer_launched: bool,
    pub installed: bool,
    pub executable_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub version: String,
    pub state: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percentage: Option<f64>,
    pub estimated: bool,
    pub message: String,
    pub downloaded_bytes_label: String,
    pub total_bytes_label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleOption {
    pub id: String,
    pub native_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalizationBundle {
    pub locale: String,
    pub preference: String,
    pub direction: String,
    pub available_locales: Vec<LocaleOption>,
    pub messages: BTreeMap<String, String>,
    pub numbers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new(code: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            message,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub label: String,
    pub executable_path: String,
}
