use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Contract versions follow semantic versioning. Changes accepted by the
/// current closed schemas may increment the minor version; serialized fields,
/// enum variants, capability IDs, or semantics require a major version.
pub const CONTRACT_SCHEMA_VERSION: &str = "4.0.0";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum BackendId {
    LinuxWinboat,
    WindowsNative,
    MacNative,
}

impl BackendId {
    pub const ALL: [Self; 3] = [Self::LinuxWinboat, Self::WindowsNative, Self::MacNative];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LinuxWinboat => "linux-winboat",
            Self::WindowsNative => "windows-native",
            Self::MacNative => "mac-native",
        }
    }
}

impl fmt::Display for BackendId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for BackendId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linux-winboat" => Ok(Self::LinuxWinboat),
            "windows-native" => Ok(Self::WindowsNative),
            "mac-native" => Ok(Self::MacNative),
            _ => Err(format!("unknown backend: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformId {
    Linux,
    Windows,
    Macos,
    Unsupported,
}

impl PlatformId {
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Unsupported
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMode {
    Portable,
    StudioRunLocally,
    ExternalUrl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityId {
    #[serde(rename = "studio.detect")]
    StudioDetect,
    #[serde(rename = "studio.install")]
    StudioInstall,
    #[serde(rename = "studio.uninstall")]
    StudioUninstall,
    #[serde(rename = "studio.start")]
    StudioStart,
    #[serde(rename = "studio.status")]
    StudioStatus,
    #[serde(rename = "studio.stop")]
    StudioStop,
    #[serde(rename = "runtime.build")]
    RuntimeBuild,
    #[serde(rename = "runtime.start")]
    RuntimeStart,
    #[serde(rename = "runtime.status")]
    RuntimeStatus,
    #[serde(rename = "runtime.wait")]
    RuntimeWait,
    #[serde(rename = "runtime.url")]
    RuntimeUrl,
    #[serde(rename = "runtime.stop")]
    RuntimeStop,
    #[serde(rename = "runtime.logs")]
    RuntimeLogs,
    #[serde(rename = "ui.capabilities")]
    UiCapabilities,
    #[serde(rename = "ui.tree")]
    UiTree,
    #[serde(rename = "ui.find")]
    UiFind,
    #[serde(rename = "ui.action")]
    UiAction,
    #[serde(rename = "ui.wait")]
    UiWait,
    #[serde(rename = "ui.screenshot")]
    UiScreenshot,
    #[serde(rename = "browser.test")]
    BrowserTest,
    #[serde(rename = "browser.artifacts")]
    BrowserArtifacts,
}

impl CapabilityId {
    pub const ALL: [Self; 21] = [
        Self::StudioDetect,
        Self::StudioInstall,
        Self::StudioUninstall,
        Self::StudioStart,
        Self::StudioStatus,
        Self::StudioStop,
        Self::RuntimeBuild,
        Self::RuntimeStart,
        Self::RuntimeStatus,
        Self::RuntimeWait,
        Self::RuntimeUrl,
        Self::RuntimeStop,
        Self::RuntimeLogs,
        Self::UiCapabilities,
        Self::UiTree,
        Self::UiFind,
        Self::UiAction,
        Self::UiWait,
        Self::UiScreenshot,
        Self::BrowserTest,
        Self::BrowserArtifacts,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::StudioDetect => "studio.detect",
            Self::StudioInstall => "studio.install",
            Self::StudioUninstall => "studio.uninstall",
            Self::StudioStart => "studio.start",
            Self::StudioStatus => "studio.status",
            Self::StudioStop => "studio.stop",
            Self::RuntimeBuild => "runtime.build",
            Self::RuntimeStart => "runtime.start",
            Self::RuntimeStatus => "runtime.status",
            Self::RuntimeWait => "runtime.wait",
            Self::RuntimeUrl => "runtime.url",
            Self::RuntimeStop => "runtime.stop",
            Self::RuntimeLogs => "runtime.logs",
            Self::UiCapabilities => "ui.capabilities",
            Self::UiTree => "ui.tree",
            Self::UiFind => "ui.find",
            Self::UiAction => "ui.action",
            Self::UiWait => "ui.wait",
            Self::UiScreenshot => "ui.screenshot",
            Self::BrowserTest => "browser.test",
            Self::BrowserArtifacts => "browser.artifacts",
        }
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendErrorCode {
    UnsupportedCapability,
    BackendMismatch,
    InvalidRequest,
    PreconditionFailed,
    OperationFailed,
    ExternalProcessTimeout,
    ExternalProcessCancelled,
    ExternalProcessInterrupted,
    ToolchainUnavailable,
    RuntimeVersionUnsupported,
    ConsistencyFailed,
    RuntimeBuildFailed,
    RuntimeInitializationFailed,
    RuntimeReadinessTimeout,
    RuntimeSessionNotFound,
    RuntimeExited,
    RuntimeGuestOffline,
    RuntimePortConflict,
    RuntimePortForwardingInvalid,
    RuntimeFirewallBlocked,
    RuntimeNotListening,
    RuntimeComposeRecoveryFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityLimitation {
    pub code: BackendErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_permission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_version: Option<String>,
}

impl CapabilityLimitation {
    pub fn not_implemented(capability: CapabilityId) -> Self {
        Self {
            code: BackendErrorCode::UnsupportedCapability,
            message: format!("{} is not implemented by this backend", capability),
            required_permission: None,
            required_version: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub id: CapabilityId,
    pub status: CapabilityStatus,
    #[serde(default)]
    pub required_permissions: Vec<String>,
    pub fallback_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitation: Option<CapabilityLimitation>,
}

impl Capability {
    pub fn supported(id: CapabilityId, permissions: &[&str]) -> Self {
        Self {
            id,
            status: CapabilityStatus::Supported,
            required_permissions: permissions
                .iter()
                .map(|permission| (*permission).to_string())
                .collect(),
            fallback_allowed: false,
            limitation: None,
        }
    }

    pub fn unsupported(id: CapabilityId, limitation: CapabilityLimitation) -> Self {
        Self {
            id,
            status: CapabilityStatus::Unsupported,
            required_permissions: Vec::new(),
            fallback_allowed: false,
            limitation: Some(limitation),
        }
    }

    pub fn with_required_permissions(mut self, permissions: &[&str]) -> Self {
        self.required_permissions = permissions
            .iter()
            .map(|permission| (*permission).to_string())
            .collect();
        if let Some(first) = self.required_permissions.first() {
            if let Some(limitation) = self.limitation.as_mut() {
                limitation.required_permission = Some(first.clone());
            }
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityManifest {
    pub schema_version: String,
    pub backend: BackendId,
    pub host_platform: PlatformId,
    pub studio_platform: PlatformId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_platform: Option<PlatformId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<RuntimeMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_modes: Vec<RuntimeMode>,
    pub architecture: String,
    pub capabilities: Vec<Capability>,
}

impl CapabilityManifest {
    pub fn capability(&self, id: CapabilityId) -> Option<&Capability> {
        self.capabilities.iter().find(|entry| entry.id == id)
    }

    pub fn supports(&self, id: CapabilityId) -> bool {
        self.capability(id)
            .is_some_and(|entry| entry.status == CapabilityStatus::Supported)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySnapshot {
    pub schema_version: String,
    pub snapshot_id: String,
    pub captured_at: DateTime<Utc>,
    pub manifest: CapabilityManifest,
}

impl CapabilitySnapshot {
    pub fn capture(manifest: CapabilityManifest) -> Result<Self, BackendError> {
        Ok(Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            snapshot_id: secure_identifier("cap")?,
            captured_at: Utc::now(),
            manifest,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Created,
    EnvironmentReady,
    StudioReady,
    ProjectReady,
    RuntimeReady,
    Testing,
    Completed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDescriptor {
    pub schema_version: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub state: SessionState,
    pub capability_snapshot: CapabilitySnapshot,
}

impl SessionDescriptor {
    pub fn create(capability_snapshot: CapabilitySnapshot) -> Result<Self, BackendError> {
        Ok(Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            session_id: secure_identifier("session")?,
            created_at: Utc::now(),
            state: SessionState::Created,
            capability_snapshot,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackendError {
    pub schema_version: String,
    pub code: BackendErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<CapabilityId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Box<CapabilityLimitation>>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_ref: Option<String>,
}

impl BackendError {
    pub fn unsupported(backend: BackendId, capability: CapabilityId) -> Self {
        let reason = CapabilityLimitation::not_implemented(capability);
        Self::unsupported_with_reason(backend, capability, reason)
    }

    pub fn unsupported_with_reason(
        backend: BackendId,
        capability: CapabilityId,
        reason: CapabilityLimitation,
    ) -> Self {
        Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            code: BackendErrorCode::UnsupportedCapability,
            message: reason.message.clone(),
            backend: Some(backend),
            capability: Some(capability),
            reason: Some(Box::new(reason)),
            retryable: false,
            diagnostic_ref: None,
        }
    }

    pub fn backend_mismatch(
        requested: BackendId,
        host_platform: PlatformId,
        expected: Option<BackendId>,
    ) -> Self {
        let expected = expected
            .map(|backend| backend.to_string())
            .unwrap_or_else(|| "none".to_string());
        Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            code: BackendErrorCode::BackendMismatch,
            message: format!(
                "backend {requested} cannot run on {host_platform:?}; expected {expected}"
            ),
            backend: Some(requested),
            capability: None,
            reason: None,
            retryable: false,
            diagnostic_ref: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            code: BackendErrorCode::InvalidRequest,
            message: message.into(),
            backend: None,
            capability: None,
            reason: None,
            retryable: false,
            diagnostic_ref: None,
        }
    }

    pub fn precondition(
        backend: BackendId,
        capability: CapabilityId,
        mut reason: CapabilityLimitation,
        retryable: bool,
    ) -> Self {
        reason.code = BackendErrorCode::PreconditionFailed;
        Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            code: BackendErrorCode::PreconditionFailed,
            message: reason.message.clone(),
            backend: Some(backend),
            capability: Some(capability),
            reason: Some(Box::new(reason)),
            retryable,
            diagnostic_ref: None,
        }
    }

    pub fn operation(
        backend: BackendId,
        capability: CapabilityId,
        message: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            code: BackendErrorCode::OperationFailed,
            message: message.into(),
            backend: Some(backend),
            capability: Some(capability),
            reason: None,
            retryable: false,
            diagnostic_ref: None,
        }
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}

pub type BackendResult<T> = Result<T, BackendError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    StudioLog,
    RuntimeLog,
    RuntimePackage,
    ConsistencyReport,
    BuildLog,
    BrowserTrace,
    BrowserReport,
    Screenshot,
    UiTree,
    DomSnapshot,
    Diagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDescriptor {
    pub schema_version: String,
    pub artifact_id: String,
    pub session_id: String,
    pub backend: BackendId,
    pub kind: ArtifactKind,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_diagnostic_ref: Option<String>,
}

impl ArtifactDescriptor {
    pub fn create(
        session_id: impl Into<String>,
        backend: BackendId,
        kind: ArtifactKind,
    ) -> Result<Self, BackendError> {
        Ok(Self {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            artifact_id: secure_identifier("artifact")?,
            session_id: session_id.into(),
            backend,
            kind,
            created_at: Utc::now(),
            media_type: None,
            location: None,
            sha256: None,
            size_bytes: None,
            backend_diagnostic_ref: None,
        })
    }
}

pub(crate) fn secure_identifier(prefix: &str) -> Result<String, BackendError> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|error| BackendError {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        code: BackendErrorCode::OperationFailed,
        message: format!("failed to generate a secure identifier: {error}"),
        backend: None,
        capability: None,
        reason: None,
        retryable: true,
        diagnostic_ref: None,
    })?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}_{suffix}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StudioSessionStatus {
    pub schema_version: String,
    pub session_id: String,
    pub version: String,
    pub state: StudioProcessState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    pub connection: StudioConnectionState,
    pub reconnectable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_unavailable: Option<StudioReconnectUnavailable>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StudioProcessState {
    Starting,
    Running,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StudioConnectionState {
    Connected,
    Disconnected,
    Native,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StudioReconnectUnavailable {
    AlreadyConnected,
    WindowUnavailable,
    ProjectReselectionRequired,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBuildRequest {
    pub session_id: String,
    pub project_path: String,
    pub required_version: String,
    pub clean: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBuildResult {
    pub session_id: String,
    pub package_artifact: ArtifactDescriptor,
    pub consistency_artifact: ArtifactDescriptor,
    pub build_log_artifact: ArtifactDescriptor,
    pub required_version: String,
    pub toolchain_version: String,
    pub cache_hit: bool,
    pub capability_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStartRequest {
    pub session_id: String,
    pub mode: RuntimeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_artifact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_port: Option<u16>,
    pub readiness_timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Starting,
    Running,
    Ready,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub schema_version: String,
    pub session_id: String,
    pub backend: BackendId,
    pub mode: RuntimeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    pub state: RuntimeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio_state: Option<StudioProcessState>,
    pub http_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<BackendErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_artifact: Option<ArtifactDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionSummary {
    pub session_id: String,
    pub backend: BackendId,
    pub mode: RuntimeMode,
    pub state: RuntimeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio_session_id: Option<String>,
    pub incompatible_record: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incompatibility_reason: Option<String>,
    pub forget_eligible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionList {
    pub schema_version: String,
    pub sessions: Vec<RuntimeSessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeForgetResult {
    pub schema_version: String,
    pub session_id: String,
    pub forgotten: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogBatch {
    pub session_id: String,
    pub entries: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationCapabilities {
    pub session_id: String,
    pub actions: Vec<UiActionKind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UiActionKind {
    Invoke,
    Click,
    Focus,
    SetValue,
    KeyboardInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiTree {
    pub session_id: String,
    pub revision: u64,
    pub root: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiFindRequest {
    pub session_id: String,
    pub role: Option<String>,
    pub name: Option<String>,
    pub automation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiElement {
    pub element_id: String,
    pub role: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiActionRequest {
    pub session_id: String,
    pub element_id: String,
    pub action: UiActionKind,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiWaitRequest {
    pub session_id: String,
    pub condition: String,
    pub timeout_milliseconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTestRequest {
    pub session_id: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_mirror_url: Option<String>,
    pub suite_path: String,
    pub runtime_context: BrowserRuntimeContext,
    pub policy: BrowserTestPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRuntimeContext {
    pub host_platform: PlatformId,
    pub studio_platform: PlatformId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_platform: Option<PlatformId>,
    pub backend: BackendId,
    pub runtime_mode: RuntimeMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTestPolicy {
    pub navigation_timeout_milliseconds: u64,
    pub action_timeout_milliseconds: u64,
    pub assertion_timeout_milliseconds: u64,
    pub fail_on_console_error: bool,
    pub fail_on_network_failure: bool,
    pub record_video: bool,
    pub record_har: bool,
    pub max_artifact_bytes: u64,
    pub retention_runs: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTestOutcome {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTestCaseSummary {
    pub name: String,
    pub outcome: BrowserTestOutcome,
    pub completed_steps: u32,
    pub total_steps: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTestSummary {
    pub schema_version: String,
    pub session_id: String,
    pub outcome: BrowserTestOutcome,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub browser_name: String,
    pub browser_version: String,
    pub playwright_version: String,
    pub tests: Vec<BrowserTestCaseSummary>,
    pub artifacts: Vec<ArtifactDescriptor>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const ENUM_REGISTRY: &str = include_str!("../../src/shared/contracts/enumValues.json");

    #[test]
    fn capability_ids_are_unique_and_stable() {
        let serialized = CapabilityId::ALL
            .into_iter()
            .map(|id| serde_json::to_string(&id).expect("capability serializes"))
            .collect::<BTreeSet<_>>();
        assert_eq!(serialized.len(), CapabilityId::ALL.len());
        assert!(serialized.contains("\"studio.detect\""));
        assert!(serialized.contains("\"browser.artifacts\""));
    }

    #[test]
    fn shared_enum_registry_matches_every_contract_variant() {
        assert_registry("backendId", BackendId::ALL);
        assert_registry(
            "platformId",
            [
                PlatformId::Linux,
                PlatformId::Windows,
                PlatformId::Macos,
                PlatformId::Unsupported,
            ],
        );
        assert_registry(
            "runtimeMode",
            [
                RuntimeMode::Portable,
                RuntimeMode::StudioRunLocally,
                RuntimeMode::ExternalUrl,
            ],
        );
        assert_registry("capabilityId", CapabilityId::ALL);
        assert_registry(
            "capabilityStatus",
            [CapabilityStatus::Supported, CapabilityStatus::Unsupported],
        );
        assert_registry(
            "backendErrorCode",
            [
                BackendErrorCode::UnsupportedCapability,
                BackendErrorCode::BackendMismatch,
                BackendErrorCode::InvalidRequest,
                BackendErrorCode::PreconditionFailed,
                BackendErrorCode::OperationFailed,
                BackendErrorCode::ExternalProcessTimeout,
                BackendErrorCode::ExternalProcessCancelled,
                BackendErrorCode::ExternalProcessInterrupted,
                BackendErrorCode::ToolchainUnavailable,
                BackendErrorCode::RuntimeVersionUnsupported,
                BackendErrorCode::ConsistencyFailed,
                BackendErrorCode::RuntimeBuildFailed,
                BackendErrorCode::RuntimeInitializationFailed,
                BackendErrorCode::RuntimeReadinessTimeout,
                BackendErrorCode::RuntimeSessionNotFound,
                BackendErrorCode::RuntimeExited,
                BackendErrorCode::RuntimeGuestOffline,
                BackendErrorCode::RuntimePortConflict,
                BackendErrorCode::RuntimePortForwardingInvalid,
                BackendErrorCode::RuntimeFirewallBlocked,
                BackendErrorCode::RuntimeNotListening,
                BackendErrorCode::RuntimeComposeRecoveryFailed,
            ],
        );
        assert_registry(
            "sessionState",
            [
                SessionState::Created,
                SessionState::EnvironmentReady,
                SessionState::StudioReady,
                SessionState::ProjectReady,
                SessionState::RuntimeReady,
                SessionState::Testing,
                SessionState::Completed,
                SessionState::Failed,
                SessionState::Blocked,
            ],
        );
        assert_registry(
            "artifactKind",
            [
                ArtifactKind::StudioLog,
                ArtifactKind::RuntimeLog,
                ArtifactKind::RuntimePackage,
                ArtifactKind::ConsistencyReport,
                ArtifactKind::BuildLog,
                ArtifactKind::BrowserTrace,
                ArtifactKind::BrowserReport,
                ArtifactKind::Screenshot,
                ArtifactKind::UiTree,
                ArtifactKind::DomSnapshot,
                ArtifactKind::Diagnostic,
            ],
        );
        assert_registry(
            "browserTestOutcome",
            [
                BrowserTestOutcome::Passed,
                BrowserTestOutcome::Failed,
                BrowserTestOutcome::Skipped,
            ],
        );
        assert_registry(
            "studioProcessState",
            [
                StudioProcessState::Starting,
                StudioProcessState::Running,
                StudioProcessState::Stopped,
                StudioProcessState::Unknown,
            ],
        );
        assert_registry(
            "studioConnectionState",
            [
                StudioConnectionState::Connected,
                StudioConnectionState::Disconnected,
                StudioConnectionState::Native,
            ],
        );
        assert_registry(
            "studioReconnectUnavailable",
            [
                StudioReconnectUnavailable::AlreadyConnected,
                StudioReconnectUnavailable::WindowUnavailable,
                StudioReconnectUnavailable::ProjectReselectionRequired,
                StudioReconnectUnavailable::Unsupported,
            ],
        );
        assert_registry(
            "runtimeState",
            [
                RuntimeState::Starting,
                RuntimeState::Running,
                RuntimeState::Ready,
                RuntimeState::Stopped,
                RuntimeState::Failed,
            ],
        );
        assert_registry(
            "uiActionKind",
            [
                UiActionKind::Invoke,
                UiActionKind::Click,
                UiActionKind::Focus,
                UiActionKind::SetValue,
                UiActionKind::KeyboardInput,
            ],
        );
    }

    #[test]
    fn session_snapshot_keeps_host_and_studio_platforms_distinct() {
        let manifest = CapabilityManifest {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            backend: BackendId::LinuxWinboat,
            host_platform: PlatformId::Linux,
            studio_platform: PlatformId::Windows,
            runtime_platform: None,
            runtime_mode: None,
            runtime_modes: Vec::new(),
            architecture: "x86_64".to_string(),
            capabilities: CapabilityId::ALL
                .into_iter()
                .map(|id| Capability::unsupported(id, CapabilityLimitation::not_implemented(id)))
                .collect(),
        };
        let snapshot = CapabilitySnapshot::capture(manifest).expect("snapshot captures");
        let session = SessionDescriptor::create(snapshot).expect("session creates");
        let json = serde_json::to_value(session).expect("session serializes");
        assert_eq!(json["schemaVersion"], CONTRACT_SCHEMA_VERSION);
        assert_eq!(
            json["capabilitySnapshot"]["manifest"]["hostPlatform"],
            "linux"
        );
        assert_eq!(
            json["capabilitySnapshot"]["manifest"]["studioPlatform"],
            "windows"
        );
        assert!(json["sessionId"]
            .as_str()
            .is_some_and(|id| id.starts_with("session_") && id.len() == 40));
    }

    #[test]
    fn unsupported_error_is_structured_and_never_retryable() {
        let error = BackendError::unsupported(BackendId::MacNative, CapabilityId::UiScreenshot);
        let json = serde_json::to_value(error).expect("error serializes");
        assert_eq!(json["code"], "unsupported_capability");
        assert_eq!(json["backend"], "mac-native");
        assert_eq!(json["capability"], "ui.screenshot");
        assert_eq!(json["retryable"], false);
        assert_eq!(
            json["reason"]["code"],
            BackendErrorCode::UnsupportedCapability.to_string_for_test()
        );
    }

    #[test]
    fn artifact_descriptor_is_portable_and_backend_scoped() {
        let artifact = ArtifactDescriptor::create(
            format!("session_{}", "ab".repeat(16)),
            BackendId::WindowsNative,
            ArtifactKind::Screenshot,
        )
        .expect("artifact creates");
        let json = serde_json::to_value(artifact).expect("artifact serializes");
        assert_eq!(json["schemaVersion"], CONTRACT_SCHEMA_VERSION);
        assert_eq!(json["backend"], "windows-native");
        assert_eq!(json["kind"], "screenshot");
        assert!(json.get("location").is_none());
        assert!(json["artifactId"]
            .as_str()
            .is_some_and(|id| id.starts_with("artifact_") && id.len() == 41));
    }

    #[test]
    fn checked_in_json_schemas_are_valid_and_version_aligned() {
        for (name, source) in [
            (
                "capabilities",
                include_str!("../../schemas/capabilities.schema.json"),
            ),
            (
                "backend-error",
                include_str!("../../schemas/backend-error.schema.json"),
            ),
            ("session", include_str!("../../schemas/session.schema.json")),
            (
                "artifact",
                include_str!("../../schemas/artifact.schema.json"),
            ),
            (
                "cli-response",
                include_str!("../../schemas/cli-response.schema.json"),
            ),
            (
                "cli-event",
                include_str!("../../schemas/cli-event.schema.json"),
            ),
            ("runtime", include_str!("../../schemas/runtime.schema.json")),
        ] {
            let schema: serde_json::Value = serde_json::from_str(source)
                .unwrap_or_else(|error| panic!("{name} schema must be valid JSON: {error}"));
            assert_eq!(
                schema["$schema"],
                "https://json-schema.org/draft/2020-12/schema"
            );
            assert_eq!(
                schema["properties"]["schemaVersion"]["const"], CONTRACT_SCHEMA_VERSION,
                "{name} schema version drifted"
            );
        }
        let browser: serde_json::Value =
            serde_json::from_str(include_str!("../../schemas/browser.schema.json"))
                .expect("browser schema must be valid JSON");
        assert_eq!(
            browser["$defs"]["summary"]["properties"]["schemaVersion"]["const"],
            CONTRACT_SCHEMA_VERSION
        );
        let suite: serde_json::Value =
            serde_json::from_str(include_str!("../../schemas/browser-suite.schema.json"))
                .expect("browser suite schema must be valid JSON");
        assert_eq!(suite["properties"]["schemaVersion"]["const"], "1.0.0");
    }

    impl BackendErrorCode {
        fn to_string_for_test(self) -> String {
            serde_json::to_value(self)
                .expect("error code serializes")
                .as_str()
                .expect("error code is a string")
                .to_string()
        }
    }

    fn assert_registry<T, I>(name: &str, values: I)
    where
        T: Serialize,
        I: IntoIterator<Item = T>,
    {
        let registry = serde_json::from_str::<serde_json::Value>(ENUM_REGISTRY)
            .expect("shared enum registry is valid JSON");
        let expected = registry[name]
            .as_object()
            .unwrap_or_else(|| panic!("enum registry entry is an object: {name}"))
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let actual = values
            .into_iter()
            .map(|value| {
                serde_json::to_value(value)
                    .expect("enum value serializes")
                    .as_str()
                    .expect("enum serializes as a string")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "shared enum contract drifted: {name}");
    }
}
