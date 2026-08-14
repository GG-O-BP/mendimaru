use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::winboat) struct WindowsOperationReport {
    pub(in crate::winboat) state: WindowsOperationState,
    #[serde(default)]
    pub(super) message: String,
    #[serde(default)]
    pub(in crate::winboat) percentage: Option<f64>,
    #[serde(default)]
    pub(in crate::winboat) estimated: bool,
    #[serde(default)]
    pub(super) timestamp: Option<String>,
    pub(super) exit_code: Option<i32>,
    pub(in crate::winboat) executable_path: Option<String>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(in crate::winboat) enum WindowsOperationState {
    Starting,
    Running,
    Staging,
    Installing,
    Finalizing,
    Verifying,
    Succeeded,
    Failed,
    #[serde(other)]
    Unknown,
}

impl WindowsOperationState {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Staging => "staging",
            Self::Installing => "installing",
            Self::Finalizing => "finalizing",
            Self::Verifying => "verifying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

pub(in crate::winboat) fn parse_install_report(
    content: &str,
) -> Result<WindowsOperationReport, serde_json::Error> {
    serde_json::from_str(content.trim_start_matches('\u{feff}').trim())
}
