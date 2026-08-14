use serde::{Deserialize, Serialize};

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
