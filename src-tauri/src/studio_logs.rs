//! Bounded, read-only diagnostics carried by the existing authenticated keeper.
use crate::contracts::BackendError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogOptions {
    pub max_bytes: u32,
    pub max_lines: u32,
    pub since: Option<String>,
    pub until: Option<String>,
    pub cursor: Option<String>,
}
impl Default for LogOptions {
    fn default() -> Self {
        Self {
            max_bytes: 65536,
            max_lines: 200,
            since: None,
            until: None,
            cursor: None,
        }
    }
}
impl LogOptions {
    pub fn validate(&self) -> Result<(), BackendError> {
        let invalid = || BackendError::invalid_request("invalid bounded Studio log request");
        if !(256..=262144).contains(&self.max_bytes) || !(1..=2000).contains(&self.max_lines) {
            return Err(invalid());
        }
        let parse = |v: &str| chrono::DateTime::parse_from_rfc3339(v).map_err(|_| invalid());
        let since = self.since.as_deref().map(parse).transpose()?;
        let until = self.until.as_deref().map(parse).transpose()?;
        if since.zip(until).is_some_and(|(s, u)| s > u) {
            return Err(invalid());
        }
        if self.cursor.as_ref().is_some_and(|s| {
            s.len() > 1024
                || s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        }) {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LogPage {
    pub session_id: String,
    pub version: String,
    pub source: String,
    pub redaction_policy: String,
    pub guest_time_zone: String,
    pub lines: Vec<String>,
    pub exception_count: u32,
    pub bytes: u32,
    pub max_bytes: u32,
    pub max_lines: u32,
    pub next_cursor: String,
    pub truncated: bool,
    pub incomplete_line: bool,
    pub initial_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub snapshot_bytes: u64,
    pub scanned_bytes: u64,
}

pub(crate) fn validate_page(
    request: &crate::ui_automation::Request,
    value: serde_json::Value,
) -> Result<LogPage, BackendError> {
    let invalid = || {
        let mut error = BackendError::operation(
            crate::contracts::BackendId::LinuxWinboat,
            crate::contracts::CapabilityId::StudioLogs,
            "studio-logs-unreadable",
        );
        error.code = crate::contracts::BackendErrorCode::PreconditionFailed;
        error
    };
    let options = request.logs.as_ref().ok_or_else(invalid)?;
    let page: LogPage = serde_json::from_value(value).map_err(|_| invalid())?;
    let bytes: usize = page.lines.iter().map(|line| line.len() + 1).sum();
    if page.session_id != request.session_id
        || page.version != "11.12.6"
        || page.source != "studio-internal"
        || page.redaction_policy != "structural-only"
        || page.lines.len() > options.max_lines as usize
        || bytes > options.max_bytes as usize
        || bytes != page.bytes as usize
        || page.max_bytes != options.max_bytes
        || page.max_lines != options.max_lines
        || page.exception_count as usize > page.lines.len()
        || page.snapshot_bytes > 16 * 1024 * 1024
        || page.scanned_bytes > 4 * 1024 * 1024
        || page.guest_time_zone.len() > 128
        || page.guest_time_zone.chars().any(char::is_control)
        || page.next_cursor.len() > 1024
        || !regex::Regex::new(r"^[A-Za-z0-9_-]+\.[0-9a-f]{64}$")
            .unwrap()
            .is_match(&page.next_cursor)
        || page
            .lines
            .iter()
            .any(|line| line.len() > 65536 || line.chars().any(char::is_control))
        || [&page.initial_timestamp, &page.last_timestamp]
            .into_iter()
            .flatten()
            .any(|stamp| {
                chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S%.f").is_err()
            })
    {
        return Err(invalid());
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relay_logs_and_foreign_pages_never_pass_as_studio_diagnostics() {
        use crate::ui_automation::{Operation, Request};
        let mut request = Request::new("studio-4242-639250850131064367", Operation::StudioLogs);
        request.logs = Some(LogOptions::default());
        assert!(validate_page(
            &request,
            serde_json::json!({"source":"runtime","lines":["ready"]})
        )
        .is_err());
        let page = serde_json::json!({
            "sessionId":request.session_id,"version":"11.12.6","source":"studio-internal","redactionPolicy":"structural-only",
            "guestTimeZone":"UTC","lines":[],"exceptionCount":0,"bytes":0,"maxBytes":65536,"maxLines":200,
            "nextCursor":format!("e30.{}", "a".repeat(64)),"truncated":false,"incompleteLine":false,"initialTimestamp":null,"lastTimestamp":null,"snapshotBytes":0,"scannedBytes":0
        });
        assert!(validate_page(&request, page.clone()).is_ok());
        for (field, value) in [
            (
                "sessionId",
                serde_json::json!("studio-9999-639250850131064368"),
            ),
            ("source", serde_json::json!("relay")),
            ("bytes", serde_json::json!(1)),
            ("snapshotBytes", serde_json::json!(16777217)),
            ("unexpected", serde_json::json!("secret")),
        ] {
            let mut invalid = page.clone();
            invalid[field] = value;
            assert!(validate_page(&request, invalid).is_err(), "{field}");
        }
    }

    #[test]
    fn bounds_and_timezones_are_required() {
        let mut options = LogOptions::default();
        assert!(options.validate().is_ok());
        options.since = Some("2026-10-20T12:00:00+09:00".into());
        options.until = Some("2026-10-20T03:01:00Z".into());
        assert!(options.validate().is_ok());
        options.until = Some("2026-10-20T02:59:00Z".into());
        assert!(options.validate().is_err());
        options.until = None;
        options.since = Some("2026-10-20T03:00:00".into());
        assert!(options.validate().is_err());
        options.since = None;
        options.max_bytes = 262145;
        assert!(options.validate().is_err());
    }
}
