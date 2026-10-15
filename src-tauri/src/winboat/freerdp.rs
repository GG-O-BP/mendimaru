use super::container::extract_version;
use super::operation::WindowsOperationFailure;
use crate::contracts::BackendErrorCode;
use crate::models::AppConfig;
use crate::process::{self, CommandPolicy};
use tokio::process::Command;

// This is a support policy, not proof that every older build fails. Debian's
// patched 3.15.0 reproduced stale Welcome pixels despite a live Sign In window;
// 3.32.2 rendered correctly. 3.24 includes upstream RAIL/timer fixes, but that
// individual boundary was not isolated by the live comparison. See #245.
pub(crate) fn supported_version(version: &str) -> bool {
    let mut components = version.split('.');
    let major = components
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    let minor = components
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    let patch = components.next().and_then(|value| {
        value
            .split(['-', '+'])
            .next()
            .and_then(|value| value.parse::<u32>().ok())
    });
    major == Some(3) && minor.is_some_and(|minor| minor >= 24) && patch.is_some()
}

pub(super) async fn ensure_supported(config: &AppConfig) -> Result<(), WindowsOperationFailure> {
    let mut command = Command::new(&config.freerdp_binary);
    command.arg("/version");
    let output = process::output(command, CommandPolicy::PROBE, None, "FreeRDP version").await?;
    let version = extract_version(if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    });
    if output.status.success() && version.as_deref().is_some_and(supported_version) {
        return Ok(());
    }
    Err(WindowsOperationFailure {
        message: crate::tr!("error-freerdp-version-unsupported"),
        exit_code: output.status.code(),
        retryable: false,
        failure_kind: None,
        backend_error_code: Some(BackendErrorCode::PreconditionFailed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remoteapp_requires_the_fixed_freerdp_three_client() {
        for version in ["3.24.0", "3.30.0", "3.32.2", "3.32.2-dev", "3.32.2+dfsg-1"] {
            assert!(supported_version(version), "{version}");
        }
        for version in ["2.11.7", "3.15.0", "3.23.0", "3.24", "4.0.0", "unknown"] {
            assert!(!supported_version(version), "{version}");
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn old_client_is_rejected_before_launch_can_touch_the_vm() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("xfreerdp3");
        std::fs::write(
            &executable,
            "#!/bin/sh\necho 'This is FreeRDP version 3.15.0'\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = AppConfig {
            freerdp_binary: executable.to_string_lossy().into_owned(),
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "must-not-be-invoked".into(),
            compose_file: directory
                .path()
                .join("missing.yml")
                .to_string_lossy()
                .into_owned(),
            container_runtime: crate::models::ContainerRuntime::Docker,
            container_name: "must-not-be-invoked".into(),
            api_url: "http://127.0.0.1:1".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 1,
            shared_directory: directory.path().to_string_lossy().into_owned(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        };
        let error = super::super::studio::launch_studio(&config, "11.12.6", "probe", None)
            .await
            .unwrap_err();
        assert_eq!(
            error.backend_error_code,
            Some(BackendErrorCode::PreconditionFailed)
        );
        assert!(!error.retryable);
    }
}
