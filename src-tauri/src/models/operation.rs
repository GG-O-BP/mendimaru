use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Install,
    Uninstall,
    Launch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl OperationState {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationStage {
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
    Launching,
    Uninstalling,
    Completed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationError {
    pub code: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRecord {
    pub schema_version: String,
    pub id: String,
    pub kind: OperationKind,
    pub target_version: String,
    pub protected_project: bool,
    pub state: OperationState,
    pub stage: OperationStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<f64>,
    pub estimated: bool,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OperationError>,
    pub retryable: bool,
    pub log_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
}
