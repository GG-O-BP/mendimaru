use serde::{Deserialize, Serialize};

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
