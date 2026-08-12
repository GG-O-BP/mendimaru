use crate::marketplace;
use crate::models::{AppConfig, DownloadProgress, InstallResult};
use crate::projects::linux_path_to_windows_share;
use crate::winboat::{install_studio, validate_version};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

const DOWNLOAD_EVENT: &str = "studio-download-progress";

#[derive(Default)]
pub struct DownloadManager {
    busy: AtomicBool,
    cancelled: AtomicBool,
    cancellable: AtomicBool,
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
            .map_err(|_| "이미 다른 Studio Pro 설치 파일을 다운로드하고 있습니다.".to_string())?;
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
) -> Result<InstallResult, String> {
    validate_version(&version)?;
    let _guard = manager.begin()?;
    let download_url = marketplace::installer_url(&version).await?;
    let installer_directory = Path::new(&config.shared_directory).join(".mendimaru/installers");
    tokio::fs::create_dir_all(&installer_directory)
        .await
        .map_err(|error| format!("설치 파일 디렉터리를 만들 수 없습니다: {error}"))?;
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
            None,
            "기존 설치 파일을 사용합니다.",
        );
        false
    } else {
        manager.cancellable.store(true, Ordering::SeqCst);
        download_file(app, manager, &version, &download_url, &installer_path).await?;
        manager.cancellable.store(false, Ordering::SeqCst);
        true
    };

    if manager.cancelled.load(Ordering::SeqCst) {
        return Err("다운로드가 취소되었습니다.".to_string());
    }
    let windows_installer_path = linux_path_to_windows_share(
        Path::new(&config.shared_directory),
        &installer_path,
        &config.windows_shared_directory,
    )?;
    emit_progress(
        app,
        &version,
        "installing",
        0,
        None,
        None,
        "WinBoat Windows에 설치하고 있습니다. 완료될 때까지 기다려 주세요.",
    );
    let executable_path = install_studio(config, &version, &windows_installer_path).await?;
    emit_progress(
        app,
        &version,
        "installed",
        0,
        None,
        Some(100.0),
        "Studio Pro 설치를 완료했습니다.",
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
) -> Result<(), String> {
    let partial = partial_path(destination);
    emit_progress(
        app,
        version,
        "connecting",
        0,
        None,
        Some(0.0),
        "Mendix 다운로드 서버에 연결하고 있습니다.",
    );
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60 * 60))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) mendimaru/0.1")
        .build()
        .map_err(|error| format!("다운로드 클라이언트를 만들 수 없습니다: {error}"))?;
    let response = client
        .get(url)
        .header("Referer", "https://marketplace.mendix.com/")
        .header("Accept", "application/octet-stream,*/*")
        .send()
        .await
        .map_err(|error| format!("Studio Pro 다운로드를 시작하지 못했습니다: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Studio Pro 다운로드 서버가 오류를 반환했습니다: {error}"))?;
    let total = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|error| format!("임시 설치 파일을 만들 수 없습니다: {error}"))?;
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
                percentage(downloaded, total),
                "다운로드를 취소했습니다.",
            );
            return Err("다운로드가 취소되었습니다.".to_string());
        }
        let bytes = chunk.map_err(|error| format!("다운로드 데이터를 읽지 못했습니다: {error}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| format!("설치 파일을 저장하지 못했습니다: {error}"))?;
        downloaded += bytes.len() as u64;
        emit_progress(
            app,
            version,
            "downloading",
            downloaded,
            total,
            percentage(downloaded, total),
            "Studio Pro 설치 파일을 다운로드하고 있습니다.",
        );
    }
    file.flush()
        .await
        .map_err(|error| format!("설치 파일 저장을 완료하지 못했습니다: {error}"))?;
    drop(file);
    tokio::fs::rename(&partial, destination)
        .await
        .map_err(|error| format!("설치 파일을 확정하지 못했습니다: {error}"))?;
    emit_progress(
        app,
        version,
        "downloaded",
        downloaded,
        total,
        Some(100.0),
        "설치 파일 다운로드를 완료했습니다.",
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
            message: message.to_string(),
        },
    );
}

fn percentage(downloaded: u64, total: Option<u64>) -> Option<f64> {
    total
        .filter(|value| *value > 0)
        .map(|value| downloaded as f64 / value as f64 * 100.0)
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
