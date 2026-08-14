use crate::marketplace;
mod cache;
mod progress;

use crate::models::{AppConfig, DownloadState};
use cache::{CacheInspection, RemoteMetadata};
use futures_util::StreamExt;
use progress::{
    emit_install_progress, emit_progress, overall_download_percentage, DownloadProgressUpdate,
    CHECKING_PROGRESS, DOWNLOAD_PROGRESS_END, DOWNLOAD_PROGRESS_START, PREPARING_PROGRESS,
    STAGING_PROGRESS_START,
};
use reqwest::header::{ETAG, LAST_MODIFIED};
use std::path::Path;
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
    force_redownload: bool,
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

    let cached_size = if force_redownload {
        emit_progress(
            app,
            DownloadProgressUpdate {
                version: &version,
                state: DownloadState::Checking,
                downloaded_bytes: 0,
                total_bytes: None,
                percentage: Some(CHECKING_PROGRESS),
                estimated: false,
                message: crate::tr!("progress-force-redownload"),
            },
        );
        None
    } else {
        match cache::inspect(&installer_path, &version, &download_url).await {
            CacheInspection::Missing => None,
            CacheInspection::Valid(metadata) => Some(metadata.size),
            CacheInspection::Invalid(error) => {
                emit_progress(
                    app,
                    DownloadProgressUpdate {
                        version: &version,
                        state: DownloadState::Checking,
                        downloaded_bytes: 0,
                        total_bytes: None,
                        percentage: Some(CHECKING_PROGRESS),
                        estimated: false,
                        message: crate::tr!("progress-cache-invalid", reason = error),
                    },
                );
                cache::discard(&installer_path)
                    .await
                    .map_err(|error| crate::tr!("error-installer-cache-remove", error = error))?;
                None
            }
        }
    };
    if let Some(size) = cached_size {
        emit_progress(
            app,
            DownloadProgressUpdate {
                version: &version,
                state: DownloadState::Ready,
                downloaded_bytes: size,
                total_bytes: Some(size),
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
            let _ = cache::discard(&installer_path).await;
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
    download_file_with_progress(
        manager,
        version,
        url,
        destination,
        |state, downloaded_bytes, total_bytes, percentage, message| {
            emit_progress(
                app,
                DownloadProgressUpdate {
                    version,
                    state,
                    downloaded_bytes,
                    total_bytes,
                    percentage,
                    estimated: false,
                    message,
                },
            );
        },
    )
    .await
}

async fn download_file_with_progress<F>(
    manager: &DownloadManager,
    version: &str,
    url: &str,
    destination: &Path,
    mut on_progress: F,
) -> Result<(), InstallError>
where
    F: FnMut(DownloadState, u64, Option<u64>, Option<f64>, String),
{
    let partial = cache::partial_path(destination);
    on_progress(
        DownloadState::Connecting,
        0,
        None,
        Some(DOWNLOAD_PROGRESS_START),
        crate::tr!("progress-connecting"),
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
    let remote_metadata = RemoteMetadata {
        content_length: total,
        etag: response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        last_modified: response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    };
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|error| crate::tr!("error-temp-installer-create", error = error))?;
    let mut downloaded = 0_u64;

    while let Some(chunk) = stream.next().await {
        if manager.cancelled.load(Ordering::SeqCst) {
            drop(file);
            let _ = tokio::fs::remove_file(&partial).await;
            on_progress(
                DownloadState::Cancelled,
                downloaded,
                total,
                overall_download_percentage(downloaded, total),
                crate::tr!("progress-cancelled"),
            );
            return Err(InstallError::Cancelled(crate::tr!(
                "error-download-cancelled"
            )));
        }
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(error) => {
                drop(file);
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(crate::tr!("error-download-data-read", error = error).into());
            }
        };
        file.write_all(&bytes)
            .await
            .map_err(|error| crate::tr!("error-installer-write", error = error))?;
        downloaded += bytes.len() as u64;
        on_progress(
            DownloadState::Downloading,
            downloaded,
            total,
            overall_download_percentage(downloaded, total),
            crate::tr!("progress-downloading"),
        );
    }
    file.flush()
        .await
        .map_err(|error| crate::tr!("error-installer-flush", error = error))?;
    file.sync_all()
        .await
        .map_err(|error| crate::tr!("error-installer-flush", error = error))?;
    drop(file);
    let validated = match cache::validate_download(&partial, total).await {
        Ok(validated) => validated,
        Err(error) => {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(crate::tr!("error-installer-download-invalid", reason = error).into());
        }
    };
    let metadata = cache::metadata_for_download(version, url, &validated, remote_metadata);
    cache::commit(&partial, destination, &metadata)
        .await
        .map_err(|error| crate::tr!("error-installer-finalize", error = error))?;
    on_progress(
        DownloadState::Downloaded,
        downloaded,
        total,
        Some(DOWNLOAD_PROGRESS_END),
        crate::tr!("progress-downloaded"),
    );
    Ok(())
}

fn safe_version_filename(version: &str) -> String {
    version
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect()
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
    use super::{
        cache, download_file_with_progress, download_user_agent, safe_version_filename,
        DownloadManager, InstallError,
    };
    use crate::models::DownloadState;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread::JoinHandle;

    struct FixtureServer {
        url: String,
        handle: JoinHandle<()>,
    }

    impl FixtureServer {
        fn finish(self) {
            self.handle.join().expect("fixture server completes");
        }
    }

    fn serve_once(body: Vec<u8>, declared_length: Option<usize>) -> FixtureServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .expect("set fixture timeout");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).expect("read fixture request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            assert!(request.starts_with(b"GET "));
            let length = declared_length.unwrap_or(body.len());
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {length}\r\nETag: \"fixture-etag\"\r\nConnection: close\r\n\r\n"
            )
            .expect("write fixture response headers");
            stream.write_all(&body).expect("write fixture body");
            stream.flush().expect("flush fixture response");
        });
        FixtureServer {
            url: format!("http://{address}/Mendix-11.12.2-Setup.exe"),
            handle,
        }
    }

    fn pe_fixture() -> Vec<u8> {
        let mut payload = vec![0_u8; 1024 * 1024 + 4096];
        payload[..2].copy_from_slice(b"MZ");
        payload[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        payload[0x80..0x84].copy_from_slice(b"PE\0\0");
        payload[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        payload
    }

    async fn run_fixture_download(
        url: &str,
        destination: &Path,
        states: &mut Vec<DownloadState>,
    ) -> Result<(), InstallError> {
        download_file_with_progress(
            &DownloadManager::default(),
            "11.12.2",
            url,
            destination,
            |state, _, _, _, _| states.push(state),
        )
        .await
    }

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

    #[test]
    #[ignore = "reads and hashes a real cached Studio Pro installer"]
    fn live_e2e_validates_a_real_studio_installer() {
        let path = std::env::var("MENDIMARU_E2E_INSTALLER_PATH")
            .expect("set MENDIMARU_E2E_INSTALLER_PATH to an official cached installer");
        let validated =
            tauri::async_runtime::block_on(cache::validate_download(Path::new(&path), None))
                .expect("the live installer must pass size, PE, and SHA-256 validation");
        assert!(validated.size > 100 * 1024 * 1024);
        assert_eq!(validated.sha256.len(), 64);
        assert!(validated
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn http_download_becomes_a_reusable_verified_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let server = serve_once(pe_fixture(), None);
        let url = server.url.clone();
        let mut states = Vec::new();

        run_fixture_download(&url, &destination, &mut states)
            .await
            .expect("download valid fixture");
        server.finish();

        assert_eq!(states.first(), Some(&DownloadState::Connecting));
        assert!(states.contains(&DownloadState::Downloading));
        assert_eq!(states.last(), Some(&DownloadState::Downloaded));
        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert!(!cache::partial_path(&destination).exists());
    }

    #[tokio::test]
    async fn http_error_payload_never_replaces_the_installer_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let server = serve_once(vec![b'X'; 1024 * 1024 + 4096], None);
        let url = server.url.clone();
        let mut states = Vec::new();

        let result = run_fixture_download(&url, &destination, &mut states).await;
        server.finish();

        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!destination.exists());
        assert!(!cache::metadata_path(&destination).exists());
        assert!(!cache::partial_path(&destination).exists());
        assert!(!states.contains(&DownloadState::Downloaded));
    }

    #[tokio::test]
    async fn truncated_http_response_never_becomes_a_cache_entry() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let payload = pe_fixture();
        let server = serve_once(payload.clone(), Some(payload.len() + 8192));
        let url = server.url.clone();
        let mut states = Vec::new();

        let result = run_fixture_download(&url, &destination, &mut states).await;
        server.finish();

        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!destination.exists());
        assert!(!cache::metadata_path(&destination).exists());
        assert!(!cache::partial_path(&destination).exists());
        assert!(!states.contains(&DownloadState::Downloaded));
    }

    #[tokio::test]
    async fn cancelled_forced_download_preserves_the_previous_verified_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = temporary.path().join("Mendix-11.12.2-Setup.exe");
        let original_server = serve_once(pe_fixture(), None);
        let original_url = original_server.url.clone();
        let mut original_states = Vec::new();
        run_fixture_download(&original_url, &destination, &mut original_states)
            .await
            .expect("download original fixture");
        original_server.finish();

        let replacement_server = serve_once(pe_fixture(), None);
        let manager = DownloadManager::default();
        manager
            .cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let result = download_file_with_progress(
            &manager,
            "11.12.2",
            &replacement_server.url,
            &destination,
            |_, _, _, _, _| {},
        )
        .await;
        replacement_server.finish();

        assert!(matches!(result, Err(InstallError::Cancelled(_))));
        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &original_url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert!(!cache::partial_path(&destination).exists());
    }
}
