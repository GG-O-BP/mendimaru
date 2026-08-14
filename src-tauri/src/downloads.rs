use crate::marketplace;
mod progress;

use crate::models::{AppConfig, DownloadState};
use crate::projects::linux_path_to_windows_share;
use crate::winboat::{install_studio, validate_version};
use futures_util::StreamExt;
use progress::{
    emit_install_progress, emit_progress, overall_download_percentage, DownloadProgressUpdate,
    CHECKING_PROGRESS, DOWNLOAD_PROGRESS_END, DOWNLOAD_PROGRESS_START, PREPARING_PROGRESS,
    STAGING_PROGRESS_START,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::AsyncWriteExt;

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
) -> Result<(), InstallError> {
    validate_version(&version)?;
    let _guard = manager.begin()?;
    emit_progress(
        app,
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Preparing,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(PREPARING_PROGRESS),
            estimated: false,
            message: crate::tr!("progress-preparing"),
        },
    );
    let download_url = marketplace::installer_url(&version).await?;
    emit_progress(
        app,
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Checking,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(CHECKING_PROGRESS),
            estimated: false,
            message: crate::tr!("progress-checking"),
        },
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
    if existing_installer {
        emit_progress(
            app,
            DownloadProgressUpdate {
                version: &version,
                state: DownloadState::Ready,
                downloaded_bytes: 0,
                total_bytes: None,
                percentage: Some(DOWNLOAD_PROGRESS_END),
                estimated: false,
                message: crate::tr!("progress-ready"),
            },
        );
    } else {
        manager.cancellable.store(true, Ordering::SeqCst);
        download_file(app, manager, &version, &download_url, &installer_path).await?;
        manager.cancellable.store(false, Ordering::SeqCst);
    }

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
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Staging,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(STAGING_PROGRESS_START),
            estimated: false,
            message: crate::tr!("progress-staging"),
        },
    );
    install_studio(config, &version, &windows_installer_path, |progress| {
        emit_install_progress(app, &version, progress)
    })
    .await?;
    emit_progress(
        app,
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Installed,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(100.0),
            estimated: false,
            message: crate::tr!("progress-installed"),
        },
    );

    Ok(())
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
        DownloadProgressUpdate {
            version,
            state: DownloadState::Connecting,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(DOWNLOAD_PROGRESS_START),
            estimated: false,
            message: crate::tr!("progress-connecting"),
        },
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
                DownloadProgressUpdate {
                    version,
                    state: DownloadState::Cancelled,
                    downloaded_bytes: downloaded,
                    total_bytes: total,
                    percentage: overall_download_percentage(downloaded, total),
                    estimated: false,
                    message: crate::tr!("progress-cancelled"),
                },
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
            DownloadProgressUpdate {
                version,
                state: DownloadState::Downloading,
                downloaded_bytes: downloaded,
                total_bytes: total,
                percentage: overall_download_percentage(downloaded, total),
                estimated: false,
                message: crate::tr!("progress-downloading"),
            },
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
        DownloadProgressUpdate {
            version,
            state: DownloadState::Downloaded,
            downloaded_bytes: downloaded,
            total_bytes: total,
            percentage: Some(DOWNLOAD_PROGRESS_END),
            estimated: false,
            message: crate::tr!("progress-downloaded"),
        },
    );
    Ok(())
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
