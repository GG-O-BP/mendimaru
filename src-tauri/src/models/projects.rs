use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectLocation {
    ConfiguredWorkspace,
    ExplicitHostSelection,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MendixProject {
    pub name: String,
    pub directory: String,
    pub mpr_path: String,
    pub windows_path: String,
    pub location: ProjectLocation,
    pub version: Option<String>,
    pub preferred_version: Option<String>,
    pub launch_pending: bool,
    pub favorite: bool,
    pub last_launched_at: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScanResult {
    pub source_key: String,
    pub projects: Vec<MendixProject>,
    pub visited_entries: usize,
    pub skipped_entries: usize,
    pub error_count: usize,
    pub errors: Vec<String>,
    pub settings_bytes_read: u64,
    pub truncated: bool,
    pub duration_ms: u64,
    pub watcher_active: bool,
}
