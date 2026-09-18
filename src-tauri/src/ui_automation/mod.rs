//! Bounded Studio UI requests using the common v5 backend/session envelope.
use crate::contracts::{BackendError, CapabilityId, UiActionKind};
#[cfg(any(target_os = "linux", test))]
use crate::contracts::{BackendErrorCode, BackendId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(target_os = "linux")]
mod bridge;

#[cfg(target_os = "linux")]
pub(crate) const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_REQUEST: u64 = 8192;
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 15000;
pub(crate) const MAX_TIMEOUT_MS: u64 = 60000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Capabilities,
    Tree,
    Find,
    Action,
    Wait,
    Screenshot,
    Release,
    Reconnect,
}
impl Operation {
    pub fn capability(self) -> CapabilityId {
        match self {
            Self::Capabilities | Self::Release | Self::Reconnect => CapabilityId::UiCapabilities,
            Self::Tree => CapabilityId::UiTree,
            Self::Find => CapabilityId::UiFind,
            Self::Action => CapabilityId::UiAction,
            Self::Wait => CapabilityId::UiWait,
            Self::Screenshot => CapabilityId::UiScreenshot,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Release => "ui.release",
            Self::Reconnect => "ui.reconnect",
            _ => self.capability().as_str(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub session_id: String,
    pub operation: Operation,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<Selector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<UiActionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    /// Physical pixels relative to the selected window: x, y, width, height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<[u32; 4]>,
}

impl Request {
    pub fn new(session_id: &str, operation: Operation) -> Self {
        Self {
            session_id: session_id.into(),
            operation,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            selector: None,
            element_id: None,
            action: None,
            value: None,
            condition: None,
            window_id: None,
            region: None,
        }
    }
    pub fn validate(&self) -> Result<(), BackendError> {
        let invalid = || BackendError::invalid_request("invalid bounded UI request");
        let mut parts = self.session_id.split('-');
        if parts.next() != Some("studio")
            || !parts.next().is_some_and(|v| {
                v.parse::<u32>().is_ok_and(|n| n > 0) && v.bytes().all(|b| b.is_ascii_digit())
            })
            || !parts.next().is_some_and(|v| {
                v.len() == 18 && v.parse::<i64>().is_ok_and(|n| n > 621355968000000000)
            })
            || parts.next().is_some()
            || !(100..=MAX_TIMEOUT_MS).contains(&self.timeout_ms)
        {
            return Err(invalid());
        }
        for text in [&self.element_id, &self.window_id].into_iter().flatten() {
            if !valid_element_id(text) {
                return Err(invalid());
            }
        }
        if let Some(selector) = &self.selector {
            if selector.role.is_none()
                && selector.name.is_none()
                && selector.automation_id.is_none()
            {
                return Err(invalid());
            }
            for text in [&selector.role, &selector.name, &selector.automation_id]
                .into_iter()
                .flatten()
            {
                if text.is_empty() || text.len() > 256 || text.chars().any(char::is_control) {
                    return Err(invalid());
                }
            }
            if selector
                .scope_id
                .as_deref()
                .is_some_and(|v| !valid_element_id(v))
            {
                return Err(invalid());
            }
        }
        if self
            .value
            .as_ref()
            .is_some_and(|v| v.len() > 256 || v.chars().any(char::is_control))
        {
            return Err(invalid());
        }
        let finding = matches!(self.operation, Operation::Find | Operation::Wait);
        if self.selector.is_some() && !finding {
            return Err(invalid());
        }
        if self.operation == Operation::Find && self.selector.is_none() {
            return Err(invalid());
        }
        if self.operation == Operation::Wait {
            if self.selector.is_some() == self.condition.is_some() {
                return Err(invalid());
            }
            if self.condition.as_deref().is_some_and(|s| {
                !matches!(
                    s,
                    "project-ready"
                        | "building"
                        | "deploying"
                        | "starting-runtime"
                        | "running"
                        | "modal"
                )
            }) {
                return Err(invalid());
            }
        } else if self.condition.is_some() {
            return Err(invalid());
        }
        if self.operation == Operation::Action {
            if self.action.is_none() || self.element_id.is_none() {
                return Err(invalid());
            }
            let requires_value = matches!(
                self.action,
                Some(UiActionKind::SetValue | UiActionKind::KeyboardInput)
            );
            if requires_value != self.value.is_some() {
                return Err(invalid());
            }
            if self.action == Some(UiActionKind::KeyboardInput)
                && !matches!(
                    self.value.as_deref(),
                    Some("Tab" | "F5" | "Ctrl+G" | "Ctrl+S" | "Enter" | "Right" | "Escape")
                )
            {
                return Err(invalid());
            }
        } else if self.action.is_some() || self.element_id.is_some() || self.value.is_some() {
            return Err(invalid());
        }
        if self.operation != Operation::Screenshot
            && (self.window_id.is_some() || self.region.is_some())
        {
            return Err(invalid());
        }
        if let Some([x, y, w, h]) = self.region {
            if w == 0
                || h == 0
                || x.checked_add(w).is_none_or(|v| v > 4096)
                || y.checked_add(h).is_none_or(|v| v > 4096)
            {
                return Err(invalid());
            }
        }
        if serde_json::to_vec(self).map_err(|_| invalid())?.len() as u64 > MAX_REQUEST {
            return Err(invalid());
        }
        Ok(())
    }
}

fn valid_element_id(value: &str) -> bool {
    value.len() <= 180
        && value.len() >= 34
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".:-".contains(&b))
}

pub(crate) fn safe_reason(reason: &str) -> Option<&'static str> {
    [
        "ui-session-unavailable",
        "ui-bridge-untrusted",
        "ui-helper-timeout",
        "ui-helper-exited",
        "ui-cancelled",
        "ui-stale-element",
        "ui-wrong-session",
        "ui-no-interactive-desktop",
        "ui-unsupported-version",
        "ui-unsupported-element",
        "ui-ambiguous-element",
        "ui-element-not-found",
        "ui-tree-truncated",
        "ui-modal-blocked",
        "ui-foreground-lost",
        "ui-effect-unverified",
        "ui-provider-failed",
        "ui-invalid-request",
        "ui-capture-failed",
        "ui-request-expired",
    ]
    .into_iter()
    .find(|v| *v == reason)
}

/// Only fixed CLR type names and a signed HRESULT may cross the CLI boundary.
/// Provider exception messages, arbitrary type names and paths remain private.
pub(crate) fn safe_diagnostic(value: &str) -> bool {
    let Some((kind, code)) = value.strip_prefix("uia:").and_then(|v| v.split_once(':')) else {
        return false;
    };
    matches!(
        kind,
        "System.Windows.Automation.ElementNotAvailableException"
            | "System.Runtime.InteropServices.COMException"
            | "System.InvalidOperationException"
            | "System.NotSupportedException"
            | "System.UnauthorizedAccessException"
            | "System.TimeoutException"
    ) && code.parse::<i32>().is_ok_and(|n| n.to_string() == code)
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn error(operation: Operation, reason: &str) -> BackendError {
    let reason = safe_reason(reason).unwrap_or("ui-provider-failed");
    let code = match reason {
        "ui-unsupported-version" | "ui-unsupported-element" => {
            BackendErrorCode::UnsupportedCapability
        }
        "ui-helper-timeout" => BackendErrorCode::ExternalProcessTimeout,
        "ui-cancelled" => BackendErrorCode::ExternalProcessCancelled,
        "ui-invalid-request" => BackendErrorCode::InvalidRequest,
        _ => BackendErrorCode::PreconditionFailed,
    };
    BackendError {
        code,
        ..BackendError::operation(BackendId::LinuxWinboat, operation.capability(), reason)
    }
}

pub(crate) async fn execute(
    config: &crate::models::AppConfig,
    request: &Request,
) -> Result<Value, BackendError> {
    request.validate()?;
    crate::platform::ui_request(config, request).await
}

#[cfg(target_os = "linux")]
pub(crate) async fn linux_request(
    config: &crate::models::AppConfig,
    request: &Request,
) -> Result<Value, BackendError> {
    request.validate()?;
    let paths = crate::app_paths::AppPaths::discover_for_cli()
        .map_err(|_| error(request.operation, "ui-session-unavailable"))?;
    if crate::winboat::registered_client_sessions()
        .iter()
        .any(|s| s.session_id == request.session_id)
    {
        let lease = crate::winboat::vm_use::acquire(
            config,
            crate::winboat::vm_use::Mode::Exclusive,
            request.operation.capability(),
        )
        .await?;
        lease.run(owned_request(config, request, None)).await
    } else {
        crate::cli::request_keeper_ui(&paths, request).await
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn owned_request(
    config: &crate::models::AppConfig,
    request: &Request,
    cancellation: Option<&crate::process::CancellationToken>,
) -> Result<Value, BackendError> {
    if request.operation != Operation::Reconnect {
        return bridge::request(request, cancellation).await;
    }
    if cancellation.is_some_and(|c| c.is_cancelled()) {
        return Err(error(request.operation, "ui-cancelled"));
    }
    let current = crate::winboat::registered_client_sessions()
        .into_iter()
        .find(|s| s.session_id == request.session_id);
    if current.is_some_and(|s| s.connection == crate::contracts::StudioConnectionState::Connected) {
        return Ok(serde_json::json!({"sessionId":request.session_id,"reconnected":false}));
    }
    // Retain the disconnected owner's authenticated monitor and project lease
    // until a replacement is verified. A failed/cancelled reconnect is not
    // evidence of Studio exit and must leave status/stop usable for this owner.
    // register_client retires the previous monitor only after successful bind.
    bounded_reconnect(
        request.timeout_ms,
        cancellation,
        crate::platform::reconnect_studio_session(config, &request.session_id),
    )
    .await?;
    Ok(serde_json::json!({"sessionId":request.session_id,"reconnected":true}))
}

#[cfg(any(target_os = "linux", test))]
async fn bounded_reconnect<E>(
    timeout_ms: u64,
    cancellation: Option<&crate::process::CancellationToken>,
    reconnect: impl std::future::Future<Output = Result<(), E>>,
) -> Result<(), BackendError> {
    let cancelled = async {
        match cancellation {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    // Dropping the reconnect future reaps its owned RemoteApp child and removes
    // its temporary mailboxes through their existing RAII guards. No Studio
    // close request is sent and protected-project records remain intact.
    tokio::select! {
        biased;
        _ = cancelled => Err(error(Operation::Reconnect, "ui-cancelled")),
        result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), reconnect) => {
            result.map_err(|_| error(Operation::Reconnect, "ui-helper-timeout"))?
                .map_err(|_| error(Operation::Reconnect, "ui-session-unavailable"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "studio-4242-639250850131064367";

    #[tokio::test]
    async fn reconnect_cancellation_and_timeout_drop_pending_owned_work() {
        for cancel in [true, false] {
            let (sender, mut receiver) = tokio::sync::oneshot::channel::<()>();
            let token = crate::process::CancellationToken::default();
            let work = async move {
                let _owned = sender;
                std::future::pending::<Result<(), ()>>().await
            };
            if cancel {
                token.cancel();
            }
            let result = bounded_reconnect(1, Some(&token), work).await.unwrap_err();
            assert_eq!(
                result.message,
                if cancel {
                    "ui-cancelled"
                } else {
                    "ui-helper-timeout"
                }
            );
            assert_eq!(
                receiver.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Closed)
            );
        }
        assert!(bounded_reconnect(100, None, async { Ok::<_, ()>(()) })
            .await
            .is_ok());
        assert_eq!(
            bounded_reconnect(100, None, async { Err(()) })
                .await
                .unwrap_err()
                .message,
            "ui-session-unavailable"
        );
    }
    #[test]
    fn bounded_requests_reject_ambiguous_or_executable_payloads() {
        for operation in [
            Operation::Capabilities,
            Operation::Tree,
            Operation::Screenshot,
            Operation::Release,
        ] {
            let request = Request::new(SESSION, operation);
            request.validate().unwrap();
            let mut bad = request.clone();
            bad.value = Some("secret".into());
            assert!(bad.validate().is_err());
            for timeout in [0, 99, 60001, u64::MAX] {
                bad = request.clone();
                bad.timeout_ms = timeout;
                assert!(bad.validate().is_err());
            }
        }
        for session in [
            "studio-0-639250850131064367",
            "studio-42-0",
            "studio-1-639250850131064367-extra",
            "studio-x-639250850131064367",
        ] {
            assert!(Request::new(session, Operation::Tree).validate().is_err());
        }
        let mut request = Request::new(SESSION, Operation::Find);
        assert!(request.validate().is_err());
        request.selector = Some(Selector {
            name: Some("Name".into()),
            ..Default::default()
        });
        request.validate().unwrap();
        let mut value = serde_json::to_value(&request).unwrap();
        value["script"] = Value::String("Start-Process anything".into());
        assert!(serde_json::from_value::<Request>(value).is_err());
        request.selector.as_mut().unwrap().name = Some("x".repeat(257));
        assert!(request.validate().is_err());
    }
    #[test]
    fn writes_require_ids_and_supported_keys_and_capture_bounds_do_not_wrap() {
        let mut r = Request::new(SESSION, Operation::Action);
        r.element_id = Some(format!("{}:1.2.3", "a".repeat(32)));
        r.action = Some(UiActionKind::KeyboardInput);
        r.value = Some("F5".into());
        r.validate().unwrap();
        r.value = Some("%{F4}".into());
        assert!(r.validate().is_err());
        r.action = Some(UiActionKind::SetValue);
        r.value = Some("line\nbreak".into());
        assert!(r.validate().is_err());
        r = Request::new(SESSION, Operation::Screenshot);
        r.region = Some([0, 0, 4096, 4096]);
        r.validate().unwrap();
        for region in [[u32::MAX, 0, 1, 1], [0, 0, 0, 1], [4096, 0, 1, 1]] {
            r.region = Some(region);
            assert!(r.validate().is_err());
        }
    }
    #[test]
    fn waits_never_guess_unknown_states_and_errors_do_not_echo_values() {
        let mut r = Request::new(SESSION, Operation::Wait);
        assert!(r.validate().is_err());
        r.condition = Some("project-ready".into());
        r.validate().unwrap();
        r.selector = Some(Selector {
            role: Some("Button".into()),
            ..Default::default()
        });
        assert!(r.validate().is_err());
        assert_eq!(
            error(Operation::Tree, "ui-secret-credential").message,
            "ui-provider-failed"
        );
        assert_eq!(
            error(Operation::Action, "ui-unsupported-element").code,
            BackendErrorCode::UnsupportedCapability
        );
        assert_eq!(
            error(Operation::Wait, "ui-helper-timeout").code,
            BackendErrorCode::ExternalProcessTimeout
        );
    }
    #[test]
    fn v5_preserves_v4_record_read_compatibility_and_rejects_older_shapes() {
        for v in ["4.0.0", "5.0.0"] {
            assert!(crate::contracts::compatible_record_schema(v));
        }
        for v in ["3.0.0", "6.0.0", "5.1.0"] {
            assert!(!crate::contracts::compatible_record_schema(v));
        }
    }
}
