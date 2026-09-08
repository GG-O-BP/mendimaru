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
    QemuBootTimeout,
    ContainerExitedDuringStartup,
    GuestStartupTimeout,
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
    #[serde(flatten)]
    pub assessment: EnvironmentAssessment,
    #[serde(default)]
    pub nvram_recovery_available: bool,
    #[serde(default)]
    pub startup: Option<crate::winboat::startup::StartupAttempt>,
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

/// Explicit policy, independent of translated messages and failure counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAssessment {
    pub connectivity: bool,
    pub readiness: EnvironmentReadiness,
    pub health: EnvironmentHealth,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReadiness {
    pub studio_launch: bool,
    pub installation: bool,
    pub uninstallation: bool,
    pub projects: bool,
    pub blocking_checks: Vec<EnvironmentDiagnosticId>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentHealth {
    pub attention_required: bool,
    pub attention_checks: Vec<EnvironmentDiagnosticId>,
}

impl EnvironmentStatus {
    pub fn assessed(mut self) -> Self {
        use EnvironmentDiagnosticId::*;
        let mut blockers = Vec::new();
        let mut attention = Vec::new();
        let mut browser_ready = false;
        for check in &self.diagnostics {
            if check.id == MarketplaceBrowser {
                browser_ready = check.status == EnvironmentDiagnosticStatus::Success;
            }
            if check.status == EnvironmentDiagnosticStatus::Success {
                continue;
            }
            attention.push(check.id);
            match check.id {
                GuestClock | MarketplaceBrowser => {}
                Winboat | Compose | ContainerRuntime | Freerdp | SharedDirectory | SharedMount
                | Container | GuestApi | Rdp => blockers.push(check.id),
            }
        }
        // Preserve platform-level preconditions, including unsupported architectures.
        self.ready = self.ready && blockers.is_empty();
        self.assessment = EnvironmentAssessment {
            connectivity: self.guest_online,
            readiness: EnvironmentReadiness {
                studio_launch: self.ready && self.platform.supports_studio_management,
                installation: self.ready && browser_ready && self.platform.supports_installation,
                uninstallation: self.ready && self.platform.supports_uninstallation,
                projects: self.ready && self.platform.supports_projects,
                blocking_checks: blockers,
            },
            health: EnvironmentHealth {
                attention_required: !attention.is_empty(),
                attention_checks: attention,
            },
        };
        self
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentDiagnosticReport<'a> {
    #[serde(flatten)]
    assessment: &'a EnvironmentAssessment,
    startup: Option<&'a crate::winboat::startup::StartupAttempt>,
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
        assessment: &status.assessment,
        startup: status.startup.as_ref(),
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

    fn healthy(native: bool) -> EnvironmentStatus {
        EnvironmentStatus {
            assessment: Default::default(),
            nvram_recovery_available: false,
            startup: None,
            platform: PlatformCapabilities {
                kind: if native {
                    HostPlatform::WindowsNative
                } else {
                    HostPlatform::LinuxWinboat
                },
                architecture: "x86_64".into(),
                requires_winboat: !native,
                supports_studio_management: true,
                supports_installation: true,
                supports_uninstallation: true,
                supports_projects: true,
            },
            ready: true,
            winboat_available: !native,
            winboat_initialized: !native,
            setup_pending: false,
            compose_available: !native,
            runtime_available: true,
            freerdp_available: !native,
            shared_directory_available: true,
            shared_mount_matches: true,
            container_status: ContainerStatus::Running,
            guest_online: true,
            diagnostics: [
                EnvironmentDiagnosticId::GuestClock,
                EnvironmentDiagnosticId::Rdp,
                EnvironmentDiagnosticId::GuestApi,
                EnvironmentDiagnosticId::SharedMount,
                EnvironmentDiagnosticId::SharedDirectory,
                EnvironmentDiagnosticId::MarketplaceBrowser,
            ]
            .into_iter()
            .map(|id| EnvironmentDiagnostic {
                id,
                status: EnvironmentDiagnosticStatus::Success,
                observed: None,
                action: None,
                error_code: None,
            })
            .collect(),
        }
    }

    #[test]
    fn clock_health_does_not_block_capabilities_but_required_paths_do() {
        for id in [
            EnvironmentDiagnosticId::GuestClock,
            EnvironmentDiagnosticId::Rdp,
            EnvironmentDiagnosticId::GuestApi,
            EnvironmentDiagnosticId::SharedMount,
        ] {
            let mut status = healthy(false);
            status
                .diagnostics
                .iter_mut()
                .find(|d| d.id == id)
                .unwrap()
                .status = EnvironmentDiagnosticStatus::Failure;
            status.guest_online = id != EnvironmentDiagnosticId::GuestApi;
            let status = status.assessed();
            let nonblocking = id == EnvironmentDiagnosticId::GuestClock;
            assert_eq!(
                status.assessment.connectivity,
                id != EnvironmentDiagnosticId::GuestApi
            );
            assert!(status.assessment.health.attention_required);
            assert_eq!(status.ready, nonblocking);
            assert_eq!(status.assessment.readiness.studio_launch, nonblocking);
            assert_eq!(status.assessment.readiness.installation, nonblocking);
            assert_eq!(status.assessment.readiness.projects, nonblocking);
            assert_eq!(
                status.assessment.readiness.blocking_checks.is_empty(),
                nonblocking
            );
        }
    }

    #[test]
    fn browser_readiness_only_blocks_installation_and_native_contract_is_preserved() {
        for native in [false, true] {
            let mut status = healthy(native);
            status
                .diagnostics
                .iter_mut()
                .find(|d| d.id == EnvironmentDiagnosticId::MarketplaceBrowser)
                .unwrap()
                .status = EnvironmentDiagnosticStatus::Warning;
            let status = status.assessed();
            assert!(status.ready && status.assessment.connectivity);
            assert!(
                status.assessment.readiness.studio_launch && status.assessment.readiness.projects
            );
            assert!(!status.assessment.readiness.installation);
            assert!(status.assessment.readiness.uninstallation);
            let json = serde_json::to_value(&status).unwrap();
            assert_eq!(json["connectivity"], true);
            assert_eq!(json["readiness"]["installation"], false);
            assert_eq!(json["health"]["attentionRequired"], true);
            assert!(json.get("assessment").is_none());
        }
    }

    #[test]
    fn report_uses_an_allowlist_and_omits_observed_values() {
        let secret = "password=hunter2 token=private-value /home/private/workspace";
        let status = EnvironmentStatus {
            assessment: Default::default(),
            nvram_recovery_available: false,
            startup: Some(crate::winboat::startup::StartupAttempt {
                id: 1,
                started_at: "2026-09-08T00:00:00Z".into(),
                phase: crate::winboat::startup::StartupPhase::StartupFailed,
                error_code: Some(crate::contracts::BackendErrorCode::QemuBootTimeout),
                container_status: ContainerStatus::Exited,
            }),
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
        assert!(report.contains("qemu_boot_timeout"));
        assert!(!report.contains("rawLog"));
        assert!(!report.contains("observed"));
        assert!(report.contains("\"schemaVersion\": \"2.0.0\""));
        assert!(report.contains("\"id\": \"winboat\""));
        assert!(report.contains("\"action\": \"redetect\""));
        assert!(report.contains("\"errorCode\": \"external-process-timeout\""));
    }
}
