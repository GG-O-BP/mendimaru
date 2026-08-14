use super::client::installed_versions;
use super::container::ensure_guest_online;
use super::operation::{
    run_windows_operation, WindowsOperationFailure, WindowsOperationRequest, WindowsOperationState,
};
use super::scripts::{install_script, launch_studio_script, uninstall_script};
use crate::models::{AppConfig, StudioInstallPhase, StudioInstallProgress};
use crate::platform::validate_version;
use crate::projects::{linux_path_to_windows_share, scan_projects};
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const STUDIO_LAUNCH_TIMEOUT_SECONDS: u64 = 5 * 60;
const INSTALL_TIMEOUT_SECONDS: u64 = 45 * 60;
const UNINSTALL_TIMEOUT_SECONDS: u64 = 15 * 60;

pub async fn launch_studio(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
    project_mpr_path: Option<&str>,
) -> Result<(), WindowsOperationFailure> {
    validate_operation_id(operation_id)?;
    ensure_guest_online(config).await?;
    let versions = installed_versions(config).await?;
    let selected = versions
        .into_iter()
        .find(|installed| installed.version == version)
        .ok_or_else(|| crate::tr!("error-studio-install-not-found", version = version))?;

    let project_argument = if let Some(project_path) = project_mpr_path {
        validate_project_argument(config, project_path)?
    } else {
        None
    };
    let label = format!("Studio Pro {}", selected.version);
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = launch_studio_script(
        &selected.executable_path,
        project_argument.as_deref(),
        &windows_report_path,
        &config.mendix_install_root,
        version,
    );
    let command = write_command_script(config, operation_id, &script)?;
    let operation = crate::tr!("operation-studio-launch");
    let mut outcome = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: &label,
            report_path: &report_path,
            timeout_seconds: STUDIO_LAUNCH_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive: true,
        },
        |_| {},
    )
    .await?;
    if outcome
        .report
        .executable_path
        .as_deref()
        .is_none_or(str::is_empty)
    {
        if let Some(mut client) = outcome.remote_app.take() {
            let _ = client.kill();
            let _ = client.wait();
        }
        return Err(crate::tr!("error-launch-path-missing").into());
    }
    let client = outcome
        .remote_app
        .take()
        .ok_or_else(|| WindowsOperationFailure::from("RemoteApp was not retained".to_string()))?;
    super::sessions::register_launch_client(version, &outcome.report.sessions, client)?;
    Ok(())
}

pub async fn install_studio<F>(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
    windows_installer_path: &str,
    expected_sha256: &str,
    mut on_progress: F,
) -> Result<String, WindowsOperationFailure>
where
    F: FnMut(StudioInstallProgress) + Send,
{
    validate_version(version)?;
    validate_operation_id(operation_id)?;
    validate_sha256(expected_sha256)?;
    ensure_guest_online(config).await?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;

    let script = install_script(
        windows_installer_path,
        &windows_report_path,
        &config.mendix_install_root,
        version,
        expected_sha256,
        &config.windows_shared_directory,
    );
    // Keep the exact script next to other commands so a failed installation can
    // be diagnosed without exposing the Windows password or FreeRDP arguments.
    let command = write_command_script(config, operation_id, &script)?;
    let label = format!("Install Studio Pro {version}");
    let operation = crate::tr!("operation-studio-install");
    on_progress(StudioInstallProgress {
        phase: StudioInstallPhase::Staging,
        percentage: Some(0.0),
        estimated: false,
    });
    let mut progress_state = InstallProgressState::default();
    let outcome = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: &label,
            report_path: &report_path,
            timeout_seconds: INSTALL_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive: false,
        },
        |report| {
            let phase = match report.state {
                WindowsOperationState::Staging => StudioInstallPhase::Staging,
                WindowsOperationState::Installing => StudioInstallPhase::Installing,
                WindowsOperationState::Finalizing => StudioInstallPhase::Finalizing,
                WindowsOperationState::Verifying => StudioInstallPhase::Verifying,
                _ => return,
            };
            progress_state.observe(phase, report.percentage, report.estimated, &mut on_progress);
        },
    )
    .await?;
    let executable_path = outcome
        .report
        .executable_path
        .filter(|path| !path.is_empty())
        .ok_or_else(|| crate::tr!("error-install-path-missing"))?;
    progress_state.complete(&mut on_progress);
    Ok(executable_path)
}

#[derive(Default)]
struct InstallProgressState {
    installing: bool,
    finalizing: bool,
    verifying: bool,
    verification_complete: bool,
}

impl InstallProgressState {
    fn observe<F>(
        &mut self,
        phase: StudioInstallPhase,
        percentage: Option<f64>,
        estimated: bool,
        on_progress: &mut F,
    ) where
        F: FnMut(StudioInstallProgress),
    {
        let Some(percentage) = percentage else {
            return;
        };
        match phase {
            StudioInstallPhase::Staging => {}
            StudioInstallPhase::Installing => self.installing = true,
            StudioInstallPhase::Finalizing => {
                self.ensure_installing(on_progress);
                self.finalizing = true;
            }
            StudioInstallPhase::Verifying => {
                self.ensure_installing(on_progress);
                self.ensure_finalizing(on_progress);
                self.verifying = true;
                self.verification_complete = percentage >= 100.0;
            }
        }
        on_progress(StudioInstallProgress {
            phase,
            percentage: Some(percentage.clamp(0.0, 100.0)),
            estimated,
        });
    }

    fn complete<F>(&mut self, on_progress: &mut F)
    where
        F: FnMut(StudioInstallProgress),
    {
        self.ensure_installing(on_progress);
        self.ensure_finalizing(on_progress);
        if !self.verifying || !self.verification_complete {
            on_progress(StudioInstallProgress {
                phase: StudioInstallPhase::Verifying,
                percentage: Some(100.0),
                estimated: false,
            });
            self.verifying = true;
            self.verification_complete = true;
        }
    }

    fn ensure_installing<F>(&mut self, on_progress: &mut F)
    where
        F: FnMut(StudioInstallProgress),
    {
        if !self.installing {
            on_progress(StudioInstallProgress {
                phase: StudioInstallPhase::Installing,
                percentage: Some(100.0),
                estimated: true,
            });
            self.installing = true;
        }
    }

    fn ensure_finalizing<F>(&mut self, on_progress: &mut F)
    where
        F: FnMut(StudioInstallProgress),
    {
        if !self.finalizing {
            on_progress(StudioInstallProgress {
                phase: StudioInstallPhase::Finalizing,
                percentage: Some(100.0),
                estimated: true,
            });
            self.finalizing = true;
        }
    }
}

pub async fn launch_uninstaller(
    config: &AppConfig,
    version: &str,
    operation_id: &str,
) -> Result<(), WindowsOperationFailure> {
    validate_version(version)?;
    validate_operation_id(operation_id)?;
    ensure_guest_online(config).await?;
    let operation_directory = secure_shared_directory(config, ".mendimaru/operations")?;
    let report_path = operation_directory.join(format!("{operation_id}.json"));
    let windows_report_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &report_path,
        &config.windows_shared_directory,
    )?;
    let script = uninstall_script(
        &config.mendix_data_root,
        &config.mendix_install_root,
        version,
        &windows_report_path,
    );
    let command = write_command_script(config, operation_id, &script)?;
    let label = format!("Uninstall Studio Pro {version}");
    let operation = crate::tr!("operation-studio-uninstall");
    run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &command.path,
            script_sha256: &command.sha256,
            label: &label,
            report_path: &report_path,
            timeout_seconds: UNINSTALL_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive: false,
        },
        |_| {},
    )
    .await?;
    Ok(())
}

pub fn open_linux_folder(path: &str) -> Result<(), String> {
    let directory = Path::new(path);
    if !directory.is_dir() {
        return Err(crate::tr!("error-directory-not-found", path = path));
    }
    Command::new("xdg-open")
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| crate::tr!("error-file-manager-open", error = error))
}

fn validate_project_argument(
    config: &AppConfig,
    requested_path: &str,
) -> Result<Option<String>, String> {
    let requested = Path::new(requested_path);
    if !requested.is_file()
        || !requested
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mpr"))
    {
        return Err(crate::tr!("error-project-not-found"));
    }
    let projects = scan_projects(config)?;
    let project = projects
        .into_iter()
        .find(|project| paths_refer_to_same_location(&project.mpr_path, requested_path))
        .ok_or_else(|| crate::tr!("error-project-not-shared"))?;
    Ok(Some(project.windows_path))
}

fn paths_refer_to_same_location(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left_path == right_path,
    }
}

pub(super) struct PreparedCommand {
    pub(super) path: PathBuf,
    pub(super) sha256: String,
}

pub(super) fn write_command_script(
    config: &AppConfig,
    name: &str,
    content: &str,
) -> Result<PreparedCommand, String> {
    let command_directory = secure_shared_directory(config, ".mendimaru/commands")?;
    let safe_name = safe_operation_name(name);
    let path = command_directory.join(format!("{safe_name}.ps1"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|error| crate::tr!("error-command-script-save", error = error))?;
    file.write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| crate::tr!("error-command-script-save", error = error))?;
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    Ok(PreparedCommand { path, sha256 })
}

pub(super) fn secure_shared_directory(
    config: &AppConfig,
    relative: &str,
) -> Result<PathBuf, String> {
    let shared = Path::new(&config.shared_directory);
    let shared_metadata = fs::symlink_metadata(shared)
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    if !shared_metadata.is_dir() || shared_metadata.file_type().is_symlink() {
        return Err(crate::tr!(
            "error-secure-shared-directory",
            error = "the configured shared root is not a direct directory"
        ));
    }
    let directory = shared.join(relative);
    fs::create_dir_all(&directory)
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    let canonical_shared = shared
        .canonicalize()
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    let canonical_directory = directory
        .canonicalize()
        .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
    if !canonical_directory.starts_with(&canonical_shared) {
        return Err(crate::tr!(
            "error-secure-shared-directory",
            error = "the application directory escapes the shared root"
        ));
    }
    let mut current = shared.to_path_buf();
    for component in Path::new(relative).components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| crate::tr!("error-secure-shared-directory", error = error))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(crate::tr!(
                "error-secure-shared-directory",
                error = "a shared application directory is a symbolic link"
            ));
        }
    }
    Ok(directory)
}

fn validate_operation_id(value: &str) -> Result<(), String> {
    if !value.is_empty() && value.len() <= 160 && safe_operation_name(value) == value {
        Ok(())
    } else {
        Err(crate::tr!("error-operation-id-invalid"))
    }
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(crate::tr!("error-installer-sha256-invalid"))
    }
}

fn safe_operation_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
        .collect()
}

#[cfg(test)]
mod progress_tests {
    use super::{validate_operation_id, InstallProgressState};
    use crate::models::{StudioInstallPhase, StudioInstallProgress};

    #[test]
    fn successful_install_synthesizes_transient_phases_missed_between_polls() {
        let mut state = InstallProgressState::default();
        let mut updates = Vec::new();
        state.complete(&mut |update| updates.push(update));

        assert_eq!(
            updates
                .iter()
                .map(|update| update.phase)
                .collect::<Vec<_>>(),
            [
                StudioInstallPhase::Installing,
                StudioInstallPhase::Finalizing,
                StudioInstallPhase::Verifying,
            ]
        );
        assert!(updates
            .iter()
            .all(|update| update.percentage == Some(100.0)));
        assert!(!updates.last().expect("verification update").estimated);
    }

    #[test]
    fn accepts_only_bounded_filename_safe_host_operation_ids() {
        assert!(validate_operation_id("install-11.12.2-0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_operation_id("").is_err());
        assert!(validate_operation_id("../operation").is_err());
        assert!(validate_operation_id(&"a".repeat(161)).is_err());
    }

    #[test]
    fn verifying_report_preserves_order_and_is_completed_exactly_once() {
        let mut state = InstallProgressState::default();
        let mut updates: Vec<StudioInstallProgress> = Vec::new();
        state.observe(
            StudioInstallPhase::Verifying,
            Some(0.0),
            false,
            &mut |update| updates.push(update),
        );
        state.complete(&mut |update| updates.push(update));
        state.complete(&mut |update| updates.push(update));

        assert_eq!(
            updates
                .iter()
                .map(|update| (update.phase, update.percentage))
                .collect::<Vec<_>>(),
            [
                (StudioInstallPhase::Installing, Some(100.0)),
                (StudioInstallPhase::Finalizing, Some(100.0)),
                (StudioInstallPhase::Verifying, Some(0.0)),
                (StudioInstallPhase::Verifying, Some(100.0)),
            ]
        );
    }
}
