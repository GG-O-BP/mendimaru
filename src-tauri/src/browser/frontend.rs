use super::*;
use crate::contracts::StudioProcessState;
use crate::process::{self, CancellationToken, CommandPolicy};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrontendState {
    Healthy,
    Unhealthy,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FrontendHealth {
    pub schema_version: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub frontend_state: FrontendState,
    pub navigation_complete: bool,
    pub document_status: Option<u16>,
    pub observation_milliseconds: u64,
    pub asset_bypass: bool,
    pub counts: FrontendCounts,
    pub truncated: bool,
    pub diagnostics: Vec<FrontendDiagnostic>,
    #[serde(default)]
    pub studio_state: Option<StudioProcessState>,
    #[serde(default)]
    pub http_ready: Option<bool>,
    #[serde(default)]
    pub runtime_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FrontendCounts {
    page_errors: u32,
    console_errors: u32,
    failed_requests: u32,
    http_errors: u32,
    error_dialogs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FrontendDiagnostic {
    code: String,
    action: String,
    occurrences: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoint: Option<FrontendEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FrontendEndpoint {
    scheme: String,
    host_kind: String,
    port: u16,
    path_kind: String,
}

const CODES: &[&str] = &[
    "shared_unc_asset_unreachable",
    "dns_failure",
    "connection_failure",
    "tls_failure",
    "request_failure",
    "http_failure",
    "mendix_widget_css_missing",
    "esm_failure",
    "page_error",
    "console_error",
    "mendix_error_dialog",
    "browser_dialog",
    "navigation_timeout",
    "observation_incomplete",
];

pub(crate) fn validate_options(navigation_ms: u64, observation_ms: u64) -> BackendResult<()> {
    if !(100..=30_000).contains(&navigation_ms) || !(100..=10_000).contains(&observation_ms) {
        return Err(BackendError::invalid_request(
            "frontend navigation timeout must be 100–30000 ms and observation must be 100–10000 ms",
        ));
    }
    Ok(())
}

// Cancellation also reaches the bounded process-tree supervisor when a UI/CLI
// caller drops its future. The task retains ownership until cleanup finishes.
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) async fn diagnose(
    backend: BackendId,
    base_url: &str,
    navigation_ms: u64,
    observation_ms: u64,
) -> BackendResult<FrontendHealth> {
    validate_options(navigation_ms, observation_ms)?;
    let failure = || {
        browser_error(
            backend,
            CapabilityId::BrowserTest,
            BackendErrorCode::OperationFailed,
            false,
        )
    };
    let prerequisite = || {
        BackendError::precondition(backend, CapabilityId::BrowserTest,
        crate::contracts::CapabilityLimitation { code: BackendErrorCode::PreconditionFailed, message: "The frontend browser could not start. Run mendimaru browser doctor --json and resolve its prerequisite checks.".into(), required_permission: None, required_version: None }, false)
    };
    let mut command = tokio::process::Command::new(node_binary().map_err(|_| prerequisite())?);
    command
        .arg(runner_path().map_err(|_| prerequisite())?)
        .arg("frontend-health")
        .env(
            "MENDIMARU_FRONTEND_REQUEST_JSON",
            serde_json::json!({
                "baseUrl": base_url, "navigationTimeoutMilliseconds": navigation_ms,
                "observationMilliseconds": observation_ms,
            })
            .to_string(),
        );
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let cancellation = CancellationToken::default();
    let _guard = CancelOnDrop(cancellation.clone());
    let output = tokio::spawn(async move {
        process::output(
            command,
            CommandPolicy::new(
                Duration::from_millis(navigation_ms + observation_ms + 15_000),
                128 * 1024,
            ),
            Some(&cancellation),
            "frontend browser diagnosis",
        )
        .await
    })
    .await
    .map_err(|_| failure())?
    .map_err(|_| failure())?;
    if output.stdout_truncated || output.stdout.iter().filter(|byte| **byte == b'\n').count() != 1 {
        return Err(failure());
    }
    let envelope: RunnerEnvelope =
        serde_json::from_slice(&output.stdout).map_err(|_| prerequisite())?;
    if !output.status.success() || !envelope.ok {
        return Err(prerequisite());
    }
    let mut report: FrontendHealth =
        serde_json::from_value(envelope.data.ok_or_else(failure)?).map_err(|_| failure())?;
    validate_report(&report, observation_ms).map_err(|_| failure())?;
    report.http_ready = report.document_status.map(|status| status < 500);
    Ok(report)
}

fn validate_report(report: &FrontendHealth, observation_ms: u64) -> Result<(), ()> {
    let counts = &report.counts;
    let incomplete = report.diagnostics.iter().any(|d| {
        matches!(
            d.code.as_str(),
            "observation_incomplete" | "navigation_timeout"
        )
    });
    let unhealthy = report.diagnostics.iter().any(|d| {
        !matches!(
            d.code.as_str(),
            "observation_incomplete" | "navigation_timeout"
        )
    });
    let expected = if unhealthy {
        FrontendState::Unhealthy
    } else if incomplete {
        FrontendState::Inconclusive
    } else {
        FrontendState::Healthy
    };
    if report.schema_version != CONTRACT_SCHEMA_VERSION
        || report.asset_bypass
        || report.observation_milliseconds != observation_ms
        || report.finished_at < report.started_at
        || report.studio_state.is_some()
        || report.http_ready.is_some()
        || report.runtime_session_id.is_some()
        || report.diagnostics.len() > 100
        || report.frontend_state != expected
        || (report.frontend_state == FrontendState::Healthy
            && (!report.navigation_complete
                || report.truncated
                || [
                    counts.page_errors,
                    counts.console_errors,
                    counts.failed_requests,
                    counts.http_errors,
                    counts.error_dialogs,
                ]
                .iter()
                .any(|n| *n != 0)
                || !report
                    .document_status
                    .is_some_and(|s| (200..400).contains(&s))))
        || report
            .document_status
            .is_some_and(|s| !(100..=599).contains(&s))
        || [
            counts.page_errors,
            counts.console_errors,
            counts.failed_requests,
            counts.http_errors,
            counts.error_dialogs,
        ]
        .iter()
        .any(|n| *n > 10_000)
    {
        return Err(());
    }
    for d in &report.diagnostics {
        if !CODES.contains(&d.code.as_str())
            || d.action.is_empty()
            || d.action.len() > 512
            || !(1..=10000).contains(&d.occurrences)
            || d.status.is_some_and(|s| !(400..=599).contains(&s))
            || d.failure.as_ref().is_some_and(|f| {
                ![
                    "dns_failure",
                    "connection_failure",
                    "tls_failure",
                    "request_failure",
                    "http_failure",
                ]
                .contains(&f.as_str())
            })
        {
            return Err(());
        }
        if let Some(e) = &d.endpoint {
            if !["http", "https", "other"].contains(&e.scheme.as_str())
                || ![
                    "shared-unc",
                    "same-origin",
                    "loopback",
                    "external",
                    "unknown",
                ]
                .contains(&e.host_kind.as_str())
                || ![
                    "shared-deployment",
                    "stylesheet",
                    "script",
                    "document",
                    "other",
                ]
                .contains(&e.path_kind.as_str())
            {
                return Err(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn healthy() -> FrontendHealth {
        serde_json::from_value(serde_json::json!({
            "schemaVersion": CONTRACT_SCHEMA_VERSION, "startedAt": "2026-09-16T00:00:00Z", "finishedAt": "2026-09-16T00:00:01Z",
            "frontendState": "healthy", "navigationComplete": true, "documentStatus": 200, "observationMilliseconds": 1000,
            "assetBypass": false, "counts": { "pageErrors": 0, "consoleErrors": 0, "failedRequests": 0, "httpErrors": 0, "errorDialogs": 0 },
            "truncated": false, "diagnostics": []
        })).unwrap()
    }
    #[test]
    fn frontend_options_are_bounded() {
        assert!(validate_options(100, 100).is_ok());
        assert!(validate_options(30000, 10000).is_ok());
        for (n, o) in [(99, 100), (30001, 100), (100, 99), (100, 10001)] {
            assert!(validate_options(n, o).is_err());
        }
    }
    #[test]
    fn runner_cannot_claim_health_for_incomplete_or_bypassed_observation() {
        let report = healthy();
        assert!(validate_report(&report, 1000).is_ok());
        for field in ["assetBypass", "truncated"] {
            let mut value = serde_json::to_value(&report).unwrap();
            value[field] = Value::Bool(true);
            assert!(validate_report(&serde_json::from_value(value).unwrap(), 1000).is_err());
        }
        let mut report = healthy();
        report.document_status = Some(404);
        assert!(validate_report(&report, 1000).is_err());
        let mut report = healthy();
        report.navigation_complete = false;
        assert!(validate_report(&report, 1000).is_err());
        let mut report = healthy();
        report.studio_state = Some(StudioProcessState::Running);
        assert!(validate_report(&report, 1000).is_err());
        let mut value = serde_json::to_value(healthy()).unwrap();
        value["rawUrl"] = Value::String("secret".into());
        assert!(serde_json::from_value::<FrontendHealth>(value).is_err());
    }
}
