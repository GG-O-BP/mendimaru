use serde::{Deserialize, Serialize};

pub const ENVIRONMENT_DIAGNOSTIC_SCHEMA_VERSION: &str = "2.0.0";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostPlatform {
    LinuxWinboat,
    WindowsNative,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub kind: HostPlatform,
    pub architecture: String,
    pub requires_winboat: bool,
    pub supports_studio_management: bool,
    pub supports_installation: bool,
    pub supports_uninstallation: bool,
    pub supports_projects: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerStatus {
    Created,
    Restarting,
    Running,
    Removing,
    Paused,
    Exited,
    Dead,
    NotFound,
    Unknown,
}

impl ContainerStatus {
    pub fn from_runtime(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "restarting" => Self::Restarting,
            "running" => Self::Running,
            "removing" => Self::Removing,
            "paused" => Self::Paused,
            "exited" => Self::Exited,
            "dead" => Self::Dead,
            "not-found" => Self::NotFound,
            _ => Self::Unknown,
        }
    }

    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    pub const fn exists(self) -> bool {
        !matches!(self, Self::NotFound)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentDiagnosticId {
    Winboat,
    Compose,
    ContainerRuntime,
    Freerdp,
    SharedDirectory,
    SharedMount,
    Container,
    GuestApi,
    GuestClock,
    Rdp,
    MarketplaceBrowser,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentDiagnosticStatus {
    Success,
    Warning,
    Failure,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentDiagnosticAction {
    Redetect,
    StartWinboat,
    OpenWinboat,
    OpenSettings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentDiagnosticErrorCode {
    ExternalProcessSpawnFailed,
    ExternalProcessTimeout,
    ExternalProcessCancelled,
    ExternalProcessInterrupted,
    GuestClockSkewExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDiagnostic {
    pub id: EnvironmentDiagnosticId,
    pub status: EnvironmentDiagnosticStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<EnvironmentDiagnosticAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<EnvironmentDiagnosticErrorCode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentStatus {
    pub platform: PlatformCapabilities,
    pub ready: bool,
    pub winboat_available: bool,
    pub winboat_initialized: bool,
    pub setup_pending: bool,
    pub compose_available: bool,
    pub runtime_available: bool,
    pub freerdp_available: bool,
    pub shared_directory_available: bool,
    pub shared_mount_matches: bool,
    pub container_status: ContainerStatus,
    pub guest_online: bool,
    pub diagnostics: Vec<EnvironmentDiagnostic>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentDiagnosticReport<'a> {
    schema_version: &'static str,
    generated_at: String,
    platform: &'a PlatformCapabilities,
    ready: bool,
    container_status: ContainerStatus,
    checks: Vec<EnvironmentDiagnosticReportCheck>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentDiagnosticReportCheck {
    id: EnvironmentDiagnosticId,
    status: EnvironmentDiagnosticStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<EnvironmentDiagnosticAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<EnvironmentDiagnosticErrorCode>,
}

pub fn environment_diagnostic_report(status: &EnvironmentStatus) -> Result<String, String> {
    let report = EnvironmentDiagnosticReport {
        schema_version: ENVIRONMENT_DIAGNOSTIC_SCHEMA_VERSION,
        generated_at: chrono::Utc::now().to_rfc3339(),
        platform: &status.platform,
        ready: status.ready,
        container_status: status.container_status,
        checks: status
            .diagnostics
            .iter()
            .map(|diagnostic| EnvironmentDiagnosticReportCheck {
                id: diagnostic.id,
                status: diagnostic.status,
                action: diagnostic.action,
                error_code: diagnostic.error_code,
            })
            .collect(),
    };
    serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not serialize environment report: {error}"))
}

#[cfg(test)]
mod diagnostic_report_tests {
    use super::*;

    #[test]
    fn report_uses_an_allowlist_and_omits_observed_values() {
        let secret = "password=hunter2 token=private-value /home/private/workspace";
        let status = EnvironmentStatus {
            platform: PlatformCapabilities {
                kind: HostPlatform::LinuxWinboat,
                architecture: "x86_64".to_string(),
                requires_winboat: true,
                supports_studio_management: true,
                supports_installation: true,
                supports_uninstallation: true,
                supports_projects: true,
            },
            ready: false,
            winboat_available: false,
            winboat_initialized: false,
            setup_pending: false,
            compose_available: false,
            runtime_available: false,
            freerdp_available: false,
            shared_directory_available: false,
            shared_mount_matches: false,
            container_status: ContainerStatus::NotFound,
            guest_online: false,
            diagnostics: vec![EnvironmentDiagnostic {
                id: EnvironmentDiagnosticId::Winboat,
                status: EnvironmentDiagnosticStatus::Failure,
                observed: Some(secret.to_string()),
                action: Some(EnvironmentDiagnosticAction::Redetect),
                error_code: Some(EnvironmentDiagnosticErrorCode::ExternalProcessTimeout),
            }],
        };

        let report = environment_diagnostic_report(&status).expect("report serializes");
        assert!(!report.contains("hunter2"));
        assert!(!report.contains("private-value"));
        assert!(!report.contains("/home/private"));
        assert!(report.contains("\"schemaVersion\": \"2.0.0\""));
        assert!(report.contains("\"id\": \"winboat\""));
        assert!(report.contains("\"action\": \"redetect\""));
        assert!(report.contains("\"errorCode\": \"external-process-timeout\""));
    }
}
