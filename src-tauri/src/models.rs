mod config;
mod download;
mod environment;
mod errors;
mod localization;
mod marketplace;
mod projects;
mod studio;

pub use config::{AppConfig, ContainerRuntime, SettingsSaveResult};
pub use download::{DownloadProgress, DownloadState};
pub use environment::{ContainerStatus, EnvironmentStatus, HostPlatform, PlatformCapabilities};
pub use errors::{CommandError, CommandErrorCode};
pub use localization::{LocaleOption, LocalizationBundle, TextDirection};
pub use marketplace::{DownloadableVersion, StudioVersionCatalog};
pub use projects::MendixProject;
#[cfg(target_os = "linux")]
pub use studio::WinApp;
pub use studio::{StudioInstallPhase, StudioInstallProgress, StudioVersion};

#[cfg(test)]
mod tests {
    use super::{
        CommandError, CommandErrorCode, ContainerRuntime, ContainerStatus, DownloadState,
        HostPlatform, TextDirection,
    };
    use std::collections::BTreeSet;

    const ENUM_REGISTRY: &str = include_str!("../../src/shared/contracts/enumValues.json");

    #[test]
    fn serializes_shared_frontend_contracts_with_expected_names() {
        assert_eq!(
            serde_json::to_string(&ContainerRuntime::Docker).expect("runtime serializes"),
            r#""docker""#
        );
        assert_eq!(
            serde_json::to_string(&ContainerStatus::NotFound).expect("status serializes"),
            r#""not-found""#
        );
        assert_eq!(
            serde_json::to_string(&DownloadState::Finalizing).expect("download state serializes"),
            r#""finalizing""#
        );
        assert_eq!(
            serde_json::to_value(CommandError::new(
                CommandErrorCode::OperationFailed,
                "failure".to_string(),
            ))
            .expect("command error serializes")["code"],
            "operation_failed"
        );
        assert_eq!(
            serde_json::to_string(&HostPlatform::WindowsNative).expect("platform serializes"),
            r#""windows-native""#
        );
    }

    #[test]
    fn normalizes_container_runtime_status_output() {
        assert_eq!(
            ContainerStatus::from_runtime(" running\n"),
            ContainerStatus::Running
        );
        assert_eq!(
            ContainerStatus::from_runtime("future-state"),
            ContainerStatus::Unknown
        );
    }

    #[test]
    fn shared_enum_registry_matches_every_serialized_rust_variant() {
        assert_registry(
            "containerRuntime",
            [ContainerRuntime::Docker, ContainerRuntime::Podman],
        );
        assert_registry(
            "containerStatus",
            [
                ContainerStatus::Created,
                ContainerStatus::Restarting,
                ContainerStatus::Running,
                ContainerStatus::Removing,
                ContainerStatus::Paused,
                ContainerStatus::Exited,
                ContainerStatus::Dead,
                ContainerStatus::NotFound,
                ContainerStatus::Unknown,
            ],
        );
        assert_registry(
            "downloadState",
            [
                DownloadState::Starting,
                DownloadState::Preparing,
                DownloadState::Checking,
                DownloadState::Connecting,
                DownloadState::Downloading,
                DownloadState::Downloaded,
                DownloadState::Ready,
                DownloadState::Staging,
                DownloadState::Installing,
                DownloadState::Finalizing,
                DownloadState::Verifying,
                DownloadState::Installed,
                DownloadState::Failed,
                DownloadState::Cancelled,
            ],
        );
        assert_registry(
            "hostPlatform",
            [
                HostPlatform::LinuxWinboat,
                HostPlatform::WindowsNative,
                HostPlatform::Unsupported,
            ],
        );
        assert_registry(
            "commandErrorCode",
            [
                CommandErrorCode::ConfigLoadFailed,
                CommandErrorCode::DownloadCancelled,
                CommandErrorCode::InstallFailed,
                CommandErrorCode::OperationFailed,
            ],
        );
        assert_registry("textDirection", [TextDirection::Ltr, TextDirection::Rtl]);
    }

    fn assert_registry<T: serde::Serialize, const N: usize>(name: &str, values: [T; N]) {
        let registry = serde_json::from_str::<serde_json::Value>(ENUM_REGISTRY)
            .expect("shared enum registry is valid JSON");
        let expected = registry[name]
            .as_object()
            .expect("enum registry entry is an object")
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
