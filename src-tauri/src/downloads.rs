use crate::marketplace;
mod progress;

use crate::models::{AppConfig, DownloadState};
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
    crate::platform::validate_version(&version)?;
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
    #[cfg(target_os = "windows")]
    {
        let verification_path = installer_path.clone();
        let verification = tokio::task::spawn_blocking(move || {
            crate::platform::verify_native_installer(&verification_path)
        })
        .await
        .map_err(|error| crate::tr!("error-native-process-join", error = error))?;
        if let Err(error) = verification {
            // This directory is an application-owned cache. Only discard a
            // payload that failed trust/integrity verification; UAC cancellation
            // and installer failures retain the already verified download.
            let _ = tokio::fs::remove_file(&installer_path).await;
            return Err(error.into());
        }
    }
    crate::platform::install_studio(config, &version, &installer_path, |progress| {
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
        .user_agent(download_user_agent())
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
    finalize_download(&partial, destination).await?;
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

async fn finalize_download(partial: &Path, destination: &Path) -> Result<(), InstallError> {
    if tokio::fs::metadata(destination).await.is_ok() {
        tokio::fs::remove_file(destination)
            .await
            .map_err(|error| crate::tr!("error-installer-finalize", error = error))?;
    }
    tokio::fs::rename(partial, destination)
        .await
        .map_err(|error| crate::tr!("error-installer-finalize", error = error))?;
    Ok(())
}

fn download_user_agent() -> String {
    let platform = if cfg!(target_os = "windows") {
        match std::env::consts::ARCH {
            "aarch64" => "Windows NT 10.0; ARM64",
            _ => "Windows NT 10.0; Win64; x64",
        }
    } else {
        "X11; Linux"
    };
    format!(
        "Mozilla/5.0 ({platform}; {architecture}) mendimaru/{}",
        env!("CARGO_PKG_VERSION"),
        architecture = std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::{download_user_agent, finalize_download, safe_version_filename};
    use std::fs;

    #[test]
    fn installer_cache_filename_rejects_path_characters() {
        assert_eq!(safe_version_filename(r"11.12.2/..\\evil"), "11.12.2..evil");
    }

    #[test]
    fn user_agent_matches_the_host_platform() {
        let agent = download_user_agent();
        assert!(agent.contains(std::env::consts::ARCH));
        if cfg!(target_os = "windows") {
            assert!(agent.contains("Windows NT 10.0"));
        } else {
            assert!(agent.contains("X11; Linux"));
        }
    }

    #[tokio::test]
    async fn completed_download_replaces_a_stale_cache_file_on_windows() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let destination = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let partial = temporary.path().join("Mendix-11.12.2-Setup.exe.download");
        fs::write(&destination, b"stale").expect("stale cache");
        fs::write(&partial, b"complete download").expect("partial download");

        finalize_download(&partial, &destination)
            .await
            .expect("finalize download");

        assert_eq!(
            fs::read(&destination).expect("final cache"),
            b"complete download"
        );
        assert!(!partial.exists());
    }
}
