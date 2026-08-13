use crate::marketplace;
use crate::models::{AppConfig, DownloadProgress, InstallResult};
use crate::projects::linux_path_to_windows_share;
use crate::winboat::{install_studio, validate_version, StudioInstallProgress};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

const DOWNLOAD_EVENT: &str = "studio-download-progress";
const PREPARING_PROGRESS: f64 = 3.0;
const CHECKING_PROGRESS: f64 = 7.0;
const DOWNLOAD_PROGRESS_START: f64 = 10.0;
const DOWNLOAD_PROGRESS_END: f64 = 58.0;
const STAGING_PROGRESS_START: f64 = 60.0;
const STAGING_PROGRESS_END: f64 = 68.0;
const INSTALL_PROGRESS_START: f64 = STAGING_PROGRESS_END;
const INSTALL_PROGRESS_END: f64 = 96.0;
const FINALIZING_PROGRESS: f64 = 97.0;
const VERIFY_PROGRESS_START: f64 = FINALIZING_PROGRESS;
const VERIFY_PROGRESS_END: f64 = 99.0;

#[derive(Default)]
pub struct DownloadManager {
    busy: AtomicBool,
    cancelled: AtomicBool,
    cancellable: AtomicBool,
}

#[derive(Debug)]
pub enum InstallError {
    Cancelled(String),
    Other(String),
}

impl From<String> for InstallError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl DownloadManager {
    pub fn cancel(&self) -> bool {
        if self.busy.load(Ordering::SeqCst) && self.cancellable.load(Ordering::SeqCst) {
            self.cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn begin(&self) -> Result<DownloadGuard<'_>, String> {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| crate::tr!("error-download-busy"))?;
        self.cancelled.store(false, Ordering::SeqCst);
        self.cancellable.store(false, Ordering::SeqCst);
        Ok(DownloadGuard { manager: self })
    }
}

struct DownloadGuard<'a> {
    manager: &'a DownloadManager,
}

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        self.manager.busy.store(false, Ordering::SeqCst);
        self.manager.cancelled.store(false, Ordering::SeqCst);
        self.manager.cancellable.store(false, Ordering::SeqCst);
    }
}

pub async fn download_and_launch(
    app: &AppHandle,
    config: &AppConfig,
    manager: &DownloadManager,
    version: String,
) -> Result<InstallResult, InstallError> {
    validate_version(&version)?;
    let _guard = manager.begin()?;
    emit_progress(
        app,
        &version,
        "preparing",
        0,
        None,
        Some(PREPARING_PROGRESS),
        false,
        &crate::tr!("progress-preparing"),
    );
    let download_url = marketplace::installer_url(&version).await?;
    emit_progress(
        app,
        &version,
        "checking",
        0,
        None,
        Some(CHECKING_PROGRESS),
        false,
        &crate::tr!("progress-checking"),
    );
    let installer_directory = Path::new(&config.shared_directory).join(".mendimaru/installers");
    tokio::fs::create_dir_all(&installer_directory)
        .await
        .map_err(|error| crate::tr!("error-installer-directory-create", error = error))?;
    let installer_path = installer_directory.join(format!(
        "Mendix-{}-Setup.exe",
        safe_version_filename(&version)
    ));

    let existing_installer = tokio::fs::metadata(&installer_path)
        .await
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 1024 * 1024);
    let downloaded = if existing_installer {
        emit_progress(
            app,
            &version,
            "ready",
            0,
            None,
            Some(DOWNLOAD_PROGRESS_END),
            false,
            &crate::tr!("progress-ready"),
        );
        false
    } else {
        manager.cancellable.store(true, Ordering::SeqCst);
        download_file(app, manager, &version, &download_url, &installer_path).await?;
        manager.cancellable.store(false, Ordering::SeqCst);
        true
    };

    if manager.cancelled.load(Ordering::SeqCst) {
        return Err(InstallError::Cancelled(crate::tr!(
            "error-download-cancelled"
        )));
    }
    let windows_installer_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &installer_path,
        &config.windows_shared_directory,
    )?;
    emit_progress(
        app,
        &version,
        "staging",
        0,
        None,
        Some(STAGING_PROGRESS_START),
        false,
        &crate::tr!("progress-staging"),
    );
    let executable_path = install_studio(config, &version, &windows_installer_path, |progress| {
        emit_install_progress(app, &version, progress)
    })
    .await?;
    emit_progress(
        app,
        &version,
        "installed",
        0,
        None,
        Some(100.0),
        false,
        &crate::tr!("progress-installed"),
    );

    Ok(InstallResult {
        version,
        installer_path: installer_path.to_string_lossy().to_string(),
        windows_installer_path,
        downloaded,
        installer_launched: true,
        installed: true,
        executable_path,
    })
}

pub fn cancel_download(manager: &DownloadManager) -> bool {
    manager.cancel()
}

async fn download_file(
    app: &AppHandle,
    manager: &DownloadManager,
    version: &str,
    url: &str,
    destination: &Path,
) -> Result<(), InstallError> {
    let partial = partial_path(destination);
    emit_progress(
        app,
        version,
        "connecting",
        0,
        None,
        Some(DOWNLOAD_PROGRESS_START),
        false,
        &crate::tr!("progress-connecting"),
    );
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60 * 60))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) mendimaru/0.1")
        .build()
        .map_err(|error| crate::tr!("error-download-client-create", error = error))?;
    let response = client
        .get(url)
        .header("Referer", "https://marketplace.mendix.com/")
        .header("Accept", "application/octet-stream,*/*")
        .send()
        .await
        .map_err(|error| crate::tr!("error-download-start", error = error))?
        .error_for_status()
        .map_err(|error| crate::tr!("error-download-server", error = error))?;
    let total = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|error| crate::tr!("error-temp-installer-create", error = error))?;
    let mut downloaded = 0_u64;

    while let Some(chunk) = stream.next().await {
        if manager.cancelled.load(Ordering::SeqCst) {
            drop(file);
            let _ = tokio::fs::remove_file(&partial).await;
            emit_progress(
                app,
                version,
                "cancelled",
                downloaded,
                total,
                overall_download_percentage(downloaded, total),
                false,
                &crate::tr!("progress-cancelled"),
            );
            return Err(InstallError::Cancelled(crate::tr!(
                "error-download-cancelled"
            )));
        }
        let bytes = chunk.map_err(|error| crate::tr!("error-download-data-read", error = error))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| crate::tr!("error-installer-write", error = error))?;
        downloaded += bytes.len() as u64;
        emit_progress(
            app,
            version,
            "downloading",
            downloaded,
            total,
            overall_download_percentage(downloaded, total),
            false,
            &crate::tr!("progress-downloading"),
        );
    }
    file.flush()
        .await
        .map_err(|error| crate::tr!("error-installer-flush", error = error))?;
    drop(file);
    tokio::fs::rename(&partial, destination)
        .await
        .map_err(|error| crate::tr!("error-installer-finalize", error = error))?;
    emit_progress(
        app,
        version,
        "downloaded",
        downloaded,
        total,
        Some(DOWNLOAD_PROGRESS_END),
        false,
        &crate::tr!("progress-downloaded"),
    );
    Ok(())
}

fn emit_progress(
    app: &AppHandle,
    version: &str,
    state: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    percentage: Option<f64>,
    estimated: bool,
    message: &str,
) {
    let _ = app.emit(
        DOWNLOAD_EVENT,
        DownloadProgress {
            version: version.to_string(),
            state: state.to_string(),
            downloaded_bytes,
            total_bytes,
            percentage,
            estimated,
            message: message.to_string(),
            downloaded_bytes_label: crate::i18n::format_bytes(downloaded_bytes),
            total_bytes_label: total_bytes.map(crate::i18n::format_bytes),
        },
    );
}

fn emit_install_progress(app: &AppHandle, version: &str, progress: StudioInstallProgress) {
    let message = match progress.state.as_str() {
        "staging" => crate::tr!("progress-staging"),
        "installing" => crate::tr!("progress-installing"),
        "finalizing" => crate::tr!("progress-finalizing"),
        "verifying" => crate::tr!("progress-verifying"),
        _ => return,
    };
    emit_progress(
        app,
        version,
        &progress.state,
        0,
        None,
        overall_install_percentage(&progress),
        progress.estimated,
        &message,
    );
}

fn overall_download_percentage(downloaded: u64, total: Option<u64>) -> Option<f64> {
    total.filter(|value| *value > 0).map(|value| {
        let downloaded_ratio = (downloaded as f64 / value as f64).clamp(0.0, 1.0);
        DOWNLOAD_PROGRESS_START
            + downloaded_ratio * (DOWNLOAD_PROGRESS_END - DOWNLOAD_PROGRESS_START)
    })
}

fn overall_install_percentage(progress: &StudioInstallProgress) -> Option<f64> {
    let phase = progress.percentage?.clamp(0.0, 100.0) / 100.0;
    Some(match progress.state.as_str() {
        "staging" => {
            STAGING_PROGRESS_START + phase * (STAGING_PROGRESS_END - STAGING_PROGRESS_START)
        }
        "installing" => {
            INSTALL_PROGRESS_START + phase * (INSTALL_PROGRESS_END - INSTALL_PROGRESS_START)
        }
        "finalizing" => FINALIZING_PROGRESS,
        "verifying" => {
            VERIFY_PROGRESS_START + phase * (VERIFY_PROGRESS_END - VERIFY_PROGRESS_START)
        }
        _ => return None,
    })
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut value = destination.as_os_str().to_os_string();
    value.push(".download");
    PathBuf::from(value)
}

fn safe_version_filename(version: &str) -> String {
    version
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        overall_download_percentage, overall_install_percentage, DOWNLOAD_PROGRESS_END,
        DOWNLOAD_PROGRESS_START, FINALIZING_PROGRESS, INSTALL_PROGRESS_END, STAGING_PROGRESS_END,
        STAGING_PROGRESS_START, VERIFY_PROGRESS_END,
    };
    use crate::winboat::StudioInstallProgress;

    #[test]
    fn download_percentage_is_mapped_to_the_overall_install_range() {
        assert_eq!(
            overall_download_percentage(0, Some(100)),
            Some(DOWNLOAD_PROGRESS_START)
        );
        assert_eq!(overall_download_percentage(50, Some(100)), Some(34.0));
        assert_eq!(
            overall_download_percentage(100, Some(100)),
            Some(DOWNLOAD_PROGRESS_END)
        );
    }

    #[test]
    fn download_percentage_handles_missing_or_invalid_totals() {
        assert_eq!(overall_download_percentage(10, None), None);
        assert_eq!(overall_download_percentage(10, Some(0)), None);
        assert_eq!(
            overall_download_percentage(120, Some(100)),
            Some(DOWNLOAD_PROGRESS_END)
        );
    }

    #[test]
    fn windows_phases_fill_the_reserved_install_ranges_without_reaching_completion() {
        let progress = |state: &str, percentage| StudioInstallProgress {
            state: state.to_string(),
            percentage: Some(percentage),
            estimated: false,
        };

        assert_eq!(
            overall_install_percentage(&progress("staging", 0.0)),
            Some(STAGING_PROGRESS_START)
        );
        assert_eq!(
            overall_install_percentage(&progress("staging", 100.0)),
            Some(STAGING_PROGRESS_END)
        );
        assert_eq!(
            overall_install_percentage(&progress("installing", 100.0)),
            Some(INSTALL_PROGRESS_END)
        );
        assert_eq!(
            overall_install_percentage(&progress("finalizing", 100.0)),
            Some(FINALIZING_PROGRESS)
        );
        assert_eq!(
            overall_install_percentage(&progress("verifying", 100.0)),
            Some(VERIFY_PROGRESS_END)
        );
        assert!(VERIFY_PROGRESS_END < 100.0);
    }
}
