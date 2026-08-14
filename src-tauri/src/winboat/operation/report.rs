use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::winboat) struct WindowsOperationReport {
    pub(in crate::winboat) state: WindowsOperationState,
    #[serde(default)]
    pub(super) message: String,
    #[serde(default)]
    pub(in crate::winboat) percentage: Option<f64>,
    #[serde(default)]
    pub(in crate::winboat) estimated: bool,
    pub(super) timestamp: String,
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
    content: &[u8],
) -> Result<WindowsOperationReport, serde_json::Error> {
    serde_json::from_slice(content.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(content))
}

#[cfg(test)]
mod tests {
    use super::parse_install_report;

    const VALID: &str = r#"{"state":"succeeded","message":"ok","percentage":null,"estimated":false,"timestamp":"2026-08-14T00:00:00Z","exitCode":0,"executablePath":null,"error":null}"#;

    #[test]
    fn requires_timestamp_and_rejects_unknown_payload_fields() {
        assert!(parse_install_report(VALID.as_bytes()).is_ok());
        assert!(parse_install_report(
            VALID
                .replace(",\"timestamp\":\"2026-08-14T00:00:00Z\"", "")
                .as_bytes()
        )
        .is_err());
        assert!(parse_install_report(VALID.replace("}", ",\"forged\":true}").as_bytes()).is_err());
    }
}
