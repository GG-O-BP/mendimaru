use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::winboat) struct WindowsOperationReport {
    pub(in crate::winboat) state: WindowsOperationState,
    #[serde(default)]
    pub(in crate::winboat) message: String,
    #[serde(default)]
    pub(in crate::winboat) percentage: Option<f64>,
    #[serde(default)]
    pub(in crate::winboat) estimated: bool,
    pub(super) timestamp: String,
    pub(super) exit_code: Option<i32>,
    pub(in crate::winboat) executable_path: Option<String>,
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(in crate::winboat) sessions: Vec<WindowsStudioSessionReport>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::winboat) struct WindowsStudioSessionReport {
    pub(in crate::winboat) session_id: String,
    pub(in crate::winboat) version: String,
    pub(in crate::winboat) process_id: u32,
    pub(in crate::winboat) started_at: String,
    #[serde(default)]
    pub(in crate::winboat) project_name: Option<String>,
    pub(in crate::winboat) has_window: bool,
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

    #[test]
    fn session_payload_is_closed_and_requires_exact_process_identity_fields() {
        let valid = r#"{"state":"succeeded","message":"ok","percentage":null,"estimated":false,"timestamp":"2026-08-15T00:00:00Z","exitCode":null,"executablePath":null,"error":null,"sessions":[{"sessionId":"studio-42-638908128000000000","version":"11.13.0","processId":42,"startedAt":"2026-08-15T00:00:00Z","projectName":"Orders","hasWindow":true}]}"#;
        let report = parse_install_report(valid.as_bytes()).expect("valid session report");
        assert_eq!(report.sessions.len(), 1);
        assert!(parse_install_report(
            valid
                .replace(
                    "\"hasWindow\":true",
                    "\"hasWindow\":true,\"path\":\"secret\""
                )
                .as_bytes()
        )
        .is_err());
        assert!(parse_install_report(
            valid
                .replace(",\"startedAt\":\"2026-08-15T00:00:00Z\"", "")
                .as_bytes()
        )
        .is_err());
    }
}
