use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DownloadState {
    Starting,
    Preparing,
    Checking,
    Connecting,
    Downloading,
    Downloaded,
    Ready,
    Staging,
    Installing,
    Finalizing,
    Verifying,
    Installed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub version: String,
    pub state: DownloadState,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percentage: Option<f64>,
    pub estimated: bool,
    pub message: String,
}
