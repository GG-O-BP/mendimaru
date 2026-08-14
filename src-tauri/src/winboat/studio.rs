use super::client::installed_versions;
use super::container::ensure_guest_online;
use super::operation::{run_windows_operation, WindowsOperationRequest, WindowsOperationState};
use super::scripts::{install_script, launch_studio_script, uninstall_script};
use crate::models::{AppConfig, DownloadState};
use crate::projects::{linux_path_to_windows_share, scan_projects};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const STUDIO_LAUNCH_TIMEOUT_SECONDS: u64 = 5 * 60;
const INSTALL_TIMEOUT_SECONDS: u64 = 45 * 60;
const UNINSTALL_TIMEOUT_SECONDS: u64 = 15 * 60;

#[derive(Debug, Clone, PartialEq)]
pub struct StudioInstallProgress {
    pub phase: StudioInstallPhase,
    pub percentage: Option<f64>,
    pub estimated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudioInstallPhase {
    Staging,
    Installing,
    Finalizing,
    Verifying,
}

impl StudioInstallPhase {
    pub const fn download_state(self) -> DownloadState {
        match self {
            Self::Staging => DownloadState::Staging,
            Self::Installing => DownloadState::Installing,
            Self::Finalizing => DownloadState::Finalizing,
            Self::Verifying => DownloadState::Verifying,
        }
    }
}

pub async fn launch_studio(
    config: &AppConfig,
    version: &str,
    project_mpr_path: Option<&str>,
) -> Result<(), String> {
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
    let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
    fs::create_dir_all(&operation_directory)
        .map_err(|error| crate::tr!("error-runtime-directory-create", error = error))?;
    let operation_id = format!(
        "launch-{}-{}",
        safe_operation_name(version),
        unix_timestamp_millis()
    );
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
    );
    let script_path = write_command_script(config, &operation_id, &script)?;
    let operation = crate::tr!("operation-studio-launch");
    let report = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &script_path,
            label: &label,
            report_path: &report_path,
            timeout_seconds: STUDIO_LAUNCH_TIMEOUT_SECONDS,
            operation: &operation,
            keep_remote_app_alive: true,
        },
        |_| {},
    )
    .await?;
    if report.executable_path.as_deref().is_none_or(str::is_empty) {
        return Err(crate::tr!("error-launch-path-missing"));
    }
    Ok(())
}

pub async fn install_studio<F>(
    config: &AppConfig,
    version: &str,
    windows_installer_path: &str,
    mut on_progress: F,
) -> Result<String, String>
where
    F: FnMut(StudioInstallProgress) + Send,
{
    validate_version(version)?;
    ensure_guest_online(config).await?;
    let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
    fs::create_dir_all(&operation_directory)
        .map_err(|error| crate::tr!("error-install-state-directory-create", error = error))?;
    let operation_id = format!(
        "install-{}-{}",
        safe_operation_name(version),
        unix_timestamp_millis()
    );
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
    );
    // Keep the exact script next to other commands so a failed installation can
    // be diagnosed without exposing the Windows password or FreeRDP arguments.
    let script_path = write_command_script(config, &operation_id, &script)?;
    let label = format!("Install Studio Pro {version}");
    let operation = crate::tr!("operation-studio-install");
    let report = run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &script_path,
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
            if report.percentage.is_some() {
                on_progress(StudioInstallProgress {
                    phase,
                    percentage: report.percentage,
                    estimated: report.estimated,
                });
            }
        },
    )
    .await?;
    report
        .executable_path
        .filter(|path| !path.is_empty())
        .ok_or_else(|| crate::tr!("error-install-path-missing"))
}

pub async fn launch_uninstaller(config: &AppConfig, version: &str) -> Result<(), String> {
    validate_version(version)?;
    ensure_guest_online(config).await?;
    let operation_directory = Path::new(&config.shared_directory).join(".mendimaru/operations");
    fs::create_dir_all(&operation_directory)
        .map_err(|error| crate::tr!("error-uninstall-state-directory-create", error = error))?;
    let operation_id = format!(
        "uninstall-{}-{}",
        safe_operation_name(version),
        unix_timestamp_millis()
    );
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
    let script_path = write_command_script(config, &operation_id, &script)?;
    let label = format!("Uninstall Studio Pro {version}");
    let operation = crate::tr!("operation-studio-uninstall");
    run_windows_operation(
        config,
        WindowsOperationRequest {
            script_path: &script_path,
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

pub fn validate_version(version: &str) -> Result<(), String> {
    let pattern = Regex::new(r"^\d+\.\d+\.\d+(?:\.\d+)?(?:-(?:beta|rc)\d*)?$")
        .map_err(|error| error.to_string())?;
    if pattern.is_match(version) {
        Ok(())
    } else {
        Err(crate::tr!("error-version-format"))
    }
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

fn write_command_script(config: &AppConfig, name: &str, content: &str) -> Result<PathBuf, String> {
    let command_directory = Path::new(&config.shared_directory).join(".mendimaru/commands");
    fs::create_dir_all(&command_directory)
        .map_err(|error| crate::tr!("error-command-directory-create", error = error))?;
    let timestamp = unix_timestamp_millis();
    let safe_name = safe_operation_name(name);
    let path = command_directory.join(format!("{safe_name}-{timestamp}.ps1"));
    fs::write(&path, content)
        .map_err(|error| crate::tr!("error-command-script-save", error = error))?;
    Ok(path)
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn safe_operation_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
        .collect()
}
