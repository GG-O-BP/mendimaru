use serde::{Deserialize, Serialize};

#[cfg_attr(target_os = "windows", allow(dead_code))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WinApp {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StudioVersion {
    pub version: String,
    pub display_name: String,
    pub executable_path: String,
    pub install_root: String,
    pub source: String,
    pub removable: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledVersionsCache {
    pub versions: Vec<StudioVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioInstallProgress {
    pub phase: StudioInstallPhase,
    pub percentage: Option<f64>,
    pub estimated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudioInstallPhase {
    Staging,
    Installing,
    Finalizing,
    Verifying,
}

impl StudioInstallPhase {
    pub const fn download_state(self) -> crate::models::DownloadState {
        match self {
            Self::Staging => crate::models::DownloadState::Staging,
            Self::Installing => crate::models::DownloadState::Installing,
            Self::Finalizing => crate::models::DownloadState::Finalizing,
            Self::Verifying => crate::models::DownloadState::Verifying,
        }
    }
}
