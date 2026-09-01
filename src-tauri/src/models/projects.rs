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
    pub last_modified: Option<String>,
}
