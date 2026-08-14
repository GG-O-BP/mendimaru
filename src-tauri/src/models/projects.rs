use serde::Serialize;

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
