use crate::marketplace;
mod cache;
mod progress;
mod queue;
pub(crate) mod storage;
pub(crate) use progress::DOWNLOAD_EVENT;
pub use queue::InstallQueue;

use crate::app_paths::AppPaths;
use crate::contracts::BackendError;
use crate::models::{AppConfig, DownloadState};
use crate::process::CancellationToken;
use cache::{CacheInspection, RemoteMetadata};
use futures_util::StreamExt;
use progress::{
    emit_install_progress, emit_progress, overall_download_percentage, DownloadProgressUpdate,
    CHECKING_PROGRESS, DOWNLOAD_PROGRESS_END, DOWNLOAD_PROGRESS_START, PREPARING_PROGRESS,
    STAGING_PROGRESS_START,
};
use reqwest::header::{CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE};
use reqwest::{redirect, Url};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub(crate) const MAX_INSTALLER_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_DOWNLOAD_REDIRECTS: usize = 5;
const MENDIX_ARTIFACT_HOST: &str = "artifacts.rnd.mendix.com";

/// Cooperative cancellation for one queued install. `discard_partial` lets a
/// user choose between retaining a resumable payload and deleting it.
#[derive(Default)]
pub struct DownloadCancellation {
    token: CancellationToken,
    discard_partial: AtomicBool,
    requested: AtomicBool,
}

impl DownloadCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self, discard_partial: bool) -> bool {
        if self.token.is_cancelled() {
            return false;
        }
        self.discard_partial
            .store(discard_partial, Ordering::SeqCst);
        self.requested.store(true, Ordering::SeqCst);
        self.token.cancel();
        true
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    pub fn token_clone(&self) -> CancellationToken {
        self.token.clone()
    }

    fn should_discard_partial(&self) -> bool {
        self.discard_partial.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
pub enum InstallError {
    Cancelled(String),
    Backend(BackendError),
    Other(String),
}

impl From<String> for InstallError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<BackendError> for InstallError {
    fn from(error: BackendError) -> Self {
        Self::Backend(error)
    }
}

pub async fn download_and_launch(
    paths: &AppPaths,
    config: &AppConfig,
    version: String,
    operation_id: &str,
    force_redownload: bool,
    cancellation: &DownloadCancellation,
    mut on_progress: impl FnMut(&crate::models::DownloadProgress) + Send,
) -> Result<(), InstallError> {
    crate::platform::validate_version(&version)?;
    emit_progress(
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Preparing,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(PREPARING_PROGRESS),
            estimated: false,
            message: crate::tr!("progress-preparing"),
        },
        &mut on_progress,
    );
    let download_url = marketplace::installer_url(&version).await?;
    let installer_cache = cache::InstallerCache::open(paths, &version)
        .map_err(|error| crate::tr!("error-installer-directory-create", error = error))?;
    let installer_path = installer_cache
        .installer_path()
        .map_err(|error| crate::tr!("error-installer-directory-create", error = error))?;
    emit_progress(
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Checking,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(CHECKING_PROGRESS),
            estimated: false,
            message: crate::tr!("progress-checking"),
        },
        &mut on_progress,
    );
    let cached_installer = if force_redownload {
        emit_progress(
            DownloadProgressUpdate {
                version: &version,
                state: DownloadState::Checking,
                downloaded_bytes: 0,
                total_bytes: None,
                percentage: Some(CHECKING_PROGRESS),
                estimated: false,
                message: crate::tr!("progress-force-redownload"),
            },
            &mut on_progress,
        );
        None
    } else {
        match cache::inspect(&installer_cache, &version, &download_url).await {
            CacheInspection::Missing => None,
            CacheInspection::Valid(metadata) => Some((metadata.size, metadata.sha256)),
            CacheInspection::Invalid(error) => {
                emit_progress(
                    DownloadProgressUpdate {
                        version: &version,
                        state: DownloadState::Checking,
                        downloaded_bytes: 0,
                        total_bytes: None,
                        percentage: Some(CHECKING_PROGRESS),
                        estimated: false,
                        message: crate::tr!("progress-cache-invalid", reason = error),
                    },
                    &mut on_progress,
                );
                cache::discard(&installer_cache)
                    .await
                    .map_err(|error| crate::tr!("error-installer-cache-remove", error = error))?;
                None
            }
        }
    };
    let installer_sha256 = if let Some((size, sha256)) = cached_installer {
        emit_progress(
            DownloadProgressUpdate {
                version: &version,
                state: DownloadState::Ready,
                downloaded_bytes: size,
                total_bytes: Some(size),
                percentage: Some(DOWNLOAD_PROGRESS_END),
                estimated: false,
                message: crate::tr!("progress-ready"),
            },
            &mut on_progress,
        );
        sha256
    } else {
        if force_redownload {
            cache::discard_partial(&installer_cache)
                .map_err(|error| crate::tr!("error-installer-cache-remove", error = error))?;
        }
        let sha256 = download_file(
            cancellation,
            &version,
            &download_url,
            &installer_cache,
            &mut on_progress,
        )
        .await?;
        sha256
    };

    if cancellation.is_cancelled() {
        return Err(InstallError::Cancelled(crate::tr!(
            "error-download-cancelled"
        )));
    }
    emit_progress(
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Staging,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(STAGING_PROGRESS_START),
            estimated: false,
            message: crate::tr!("progress-staging"),
        },
        &mut on_progress,
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
            let _ = cache::discard(&installer_cache).await;
            return Err(error.into());
        }
    }
    let installation = crate::platform::install_studio(
        config,
        &version,
        operation_id,
        &installer_path,
        &installer_sha256,
        cancellation.token_clone(),
        |progress| emit_install_progress(&version, progress, &mut on_progress),
    )
    .await;
    installation?;
    emit_progress(
        DownloadProgressUpdate {
            version: &version,
            state: DownloadState::Installed,
            downloaded_bytes: 0,
            total_bytes: None,
            percentage: Some(100.0),
            estimated: false,
            message: crate::tr!("progress-installed"),
        },
        &mut on_progress,
    );

    Ok(())
}

async fn download_file<F>(
    cancellation: &DownloadCancellation,
    version: &str,
    url: &str,
    destination: &cache::InstallerCache,
    on_operation_progress: &mut F,
) -> Result<String, InstallError>
where
    F: FnMut(&crate::models::DownloadProgress),
{
    download_file_with_progress(
        cancellation,
        version,
        url,
        destination,
        DownloadPolicy::production(),
        |state, downloaded_bytes, total_bytes, percentage, message| {
            emit_progress(
                DownloadProgressUpdate {
                    version,
                    state,
                    downloaded_bytes,
                    total_bytes,
                    percentage,
                    estimated: false,
                    message,
                },
                on_operation_progress,
            );
        },
    )
    .await
}

async fn download_file_with_progress<F>(
    cancellation: &DownloadCancellation,
    version: &str,
    url: &str,
    destination: &cache::InstallerCache,
    policy: DownloadPolicy,
    mut on_progress: F,
) -> Result<String, InstallError>
where
    F: FnMut(DownloadState, u64, Option<u64>, Option<f64>, String),
{
    let parsed_url = Url::parse(url).map_err(|_| crate::tr!("error-download-source-untrusted"))?;
    policy.validate(&parsed_url)?;
    let redirect_policy = policy.redirect_policy();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60 * 60))
        .user_agent(download_user_agent())
        .redirect(redirect_policy)
        .build()
        .map_err(|error| crate::tr!("error-download-client-create", error = error))?;

    let resume = cache::load_partial(destination, version, url, policy.maximum_bytes).await;
    let start = resume
        .as_ref()
        .map(|state| state.payload_bytes)
        .unwrap_or(0);
    if resume.is_none() {
        let _ = cache::discard_partial(destination);
    }
    let validator = resume
        .as_ref()
        .and_then(|state| state.validator().map(str::to_string));

    let mut request = client
        .get(parsed_url)
        .header("Referer", "https://marketplace.mendix.com/")
        .header("Accept", "application/octet-stream,*/*");
    if start > 0 {
        request = request.header(RANGE, format!("bytes={start}-"));
        if let Some(validator) = validator.as_deref() {
            request = request.header(IF_RANGE, validator);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|error| crate::tr!("error-download-start", error = error))?
        .error_for_status()
        .map_err(|error| crate::tr!("error-download-server", error = error))?;

    let mut remote_metadata = remote_metadata_from(&response);
    let resume_offset = match response.status() {
        reqwest::StatusCode::PARTIAL_CONTENT => {
            let Some((range_start, range_total)) = parse_content_range(&response) else {
                let _ = cache::discard_partial(destination);
                return Box::pin(download_file_with_progress(
                    cancellation,
                    version,
                    url,
                    destination,
                    policy,
                    on_progress,
                ))
                .await;
            };
            if range_start > start || range_total.is_some_and(|total| total > policy.maximum_bytes)
            {
                let _ = cache::discard_partial(destination);
                return Box::pin(download_file_with_progress(
                    cancellation,
                    version,
                    url,
                    destination,
                    policy,
                    on_progress,
                ))
                .await;
            }
            if let Some(total) = range_total {
                remote_metadata.content_length = Some(total);
            }
            range_start
        }
        _ => {
            if start > 0 {
                on_progress(
                    DownloadState::Connecting,
                    0,
                    None,
                    Some(DOWNLOAD_PROGRESS_START),
                    crate::tr!("progress-download-restarted"),
                );
                let _ = cache::discard_partial(destination);
            }
            0
        }
    };
    let total = remote_metadata.content_length;
    if total.is_some_and(|size| size > policy.maximum_bytes) {
        return Err(crate::tr!(
            "error-installer-download-too-large",
            limit = policy.maximum_bytes
        )
        .into());
    }
    if resume_offset > 0 {
        on_progress(
            DownloadState::Connecting,
            resume_offset,
            total,
            Some(DOWNLOAD_PROGRESS_START),
            crate::tr!("progress-download-resumed"),
        );
    } else {
        on_progress(
            DownloadState::Connecting,
            0,
            None,
            Some(DOWNLOAD_PROGRESS_START),
            crate::tr!("progress-connecting"),
        );
    }

    let mut payload = destination
        .open_partial_payload()
        .map_err(|error| crate::tr!("error-temp-installer-create", error = error))?;
    let payload_len = payload
        .metadata()
        .await
        .map_err(|error| crate::tr!("error-installer-write", error = error))?
        .len();
    if payload_len != resume_offset {
        payload
            .set_len(resume_offset)
            .await
            .map_err(|error| crate::tr!("error-installer-write", error = error))?;
        payload
            .flush()
            .await
            .map_err(|error| crate::tr!("error-installer-flush", error = error))?;
    }

    let mut received: u64 = 0;
    let mut stream = response.bytes_stream();
    let result = loop {
        let chunk = stream.next().await;
        let Some(chunk) = chunk else {
            break Ok(());
        };
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(error) => {
                break Err(Interrupted::Network(crate::tr!(
                    "error-download-data-read",
                    error = error
                )));
            }
        };
        if cancellation.is_cancelled() {
            break Err(Interrupted::Cancelled);
        }
        let next_size = resume_offset
            .checked_add(received)
            .and_then(|size| size.checked_add(bytes.len() as u64))
            .filter(|size| *size <= policy.maximum_bytes);
        let Some(next_size) = next_size else {
            break Err(Interrupted::TooLarge);
        };
        if let Err(error) = payload.write_all(&bytes).await {
            break Err(Interrupted::Network(crate::tr!(
                "error-installer-write",
                error = error
            )));
        }
        received = next_size - resume_offset;
        on_progress(
            DownloadState::Downloading,
            next_size,
            total,
            overall_download_percentage(next_size, total),
            crate::tr!("progress-downloading"),
        );
    };

    let downloaded = resume_offset + received;
    match result {
        Err(Interrupted::Cancelled) => {
            persist_partial(
                destination,
                version,
                url,
                &mut payload,
                downloaded,
                &remote_metadata,
                cancellation.should_discard_partial(),
            )
            .await;
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
        Err(Interrupted::Network(message)) => {
            persist_partial(
                destination,
                version,
                url,
                &mut payload,
                downloaded,
                &remote_metadata,
                false,
            )
            .await;
            return Err(InstallError::Other(message));
        }
        Err(Interrupted::TooLarge) => {
            let _ = cache::discard_partial(destination);
            return Err(InstallError::Other(crate::tr!(
                "error-installer-download-too-large",
                limit = policy.maximum_bytes
            )));
        }
        Ok(()) => {}
    }

    payload
        .flush()
        .await
        .map_err(|error| crate::tr!("error-installer-flush", error = error))?;
    payload
        .sync_all()
        .await
        .map_err(|error| crate::tr!("error-installer-flush", error = error))?;
    let mut payload = payload;
    let validated = match cache::validate_partial_payload(&mut payload, total).await {
        Ok(validated) => validated,
        Err(error) => {
            let _ = cache::discard_partial(destination);
            return Err(crate::tr!("error-installer-download-invalid", reason = error).into());
        }
    };
    let metadata = cache::metadata_for_download(version, url, &validated, remote_metadata);
    cache::commit_named(destination, &destination.partial_payload_name(), &metadata)
        .await
        .map_err(|error| crate::tr!("error-installer-finalize", error = error))?;
    cache::discard_partial(destination)
        .map_err(|error| crate::tr!("error-installer-finalize", error = error))?;
    on_progress(
        DownloadState::Downloaded,
        downloaded,
        total,
        Some(DOWNLOAD_PROGRESS_END),
        crate::tr!("progress-downloaded"),
    );
    Ok(validated.sha256)
}

enum Interrupted {
    Cancelled,
    Network(String),
    TooLarge,
}

fn remote_metadata_from(response: &reqwest::Response) -> RemoteMetadata {
    RemoteMetadata {
        content_length: response.content_length(),
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
    }
}

fn parse_content_range(response: &reqwest::Response) -> Option<(u64, Option<u64>)> {
    let value = response.headers().get(CONTENT_RANGE)?.to_str().ok()?.trim();
    let value = value.strip_prefix("bytes ")?.trim();
    let (range, total) = value.split_once('/')?;
    let total = if total.trim() == "*" {
        None
    } else {
        Some(total.trim().parse::<u64>().ok()?)
    };
    let (start, _end) = range.trim().split_once('-')?;
    Some((start.trim().parse::<u64>().ok()?, total))
}

async fn persist_partial(
    destination: &cache::InstallerCache,
    version: &str,
    url: &str,
    payload: &mut tokio::fs::File,
    payload_bytes: u64,
    remote: &RemoteMetadata,
    discard: bool,
) {
    if discard {
        let _ = cache::discard_partial(destination);
        return;
    }
    let actual_bytes = payload
        .metadata()
        .await
        .ok()
        .map(|metadata| metadata.len())
        .unwrap_or(payload_bytes);
    if actual_bytes == 0 {
        let _ = cache::discard_partial(destination);
        return;
    }
    if let (Err(_), Err(_)) = (payload.flush().await, payload.sync_all().await) {
        return;
    }
    let state = cache::PartialDownloadState {
        schema_version: 1,
        version: version.to_string(),
        source_url: url.to_string(),
        payload_bytes: actual_bytes,
        total_bytes: remote.content_length,
        etag: remote.etag.clone(),
        last_modified: remote.last_modified.clone(),
    };
    if state.validator().is_none() {
        let _ = cache::discard_partial(destination);
        return;
    }
    let _ = cache::save_partial(destination, &state).await;
}

#[derive(Clone)]
struct DownloadPolicy {
    scheme: &'static str,
    host: &'static str,
    port: u16,
    maximum_bytes: u64,
    maximum_redirects: usize,
}

impl DownloadPolicy {
    fn production() -> Self {
        Self {
            scheme: "https",
            host: MENDIX_ARTIFACT_HOST,
            port: 443,
            maximum_bytes: MAX_INSTALLER_BYTES,
            maximum_redirects: MAX_DOWNLOAD_REDIRECTS,
        }
    }

    fn validate(&self, url: &Url) -> Result<(), InstallError> {
        if url.scheme() != self.scheme
            || !url
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case(self.host))
            || url.port_or_known_default() != Some(self.port)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(crate::tr!("error-download-source-untrusted").into());
        }
        Ok(())
    }

    fn redirect_policy(&self) -> redirect::Policy {
        let policy = self.clone();
        redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > policy.maximum_redirects {
                return attempt.error("the installer redirect limit was exceeded");
            }
            if policy.validate(attempt.url()).is_err() {
                return attempt.error("the installer redirect target is not trusted");
            }
            attempt.follow()
        })
    }

    #[cfg(test)]
    fn fixture(url: &str, maximum_bytes: u64) -> Self {
        let url = Url::parse(url).expect("fixture URL");
        Self {
            scheme: if url.scheme() == "https" {
                "https"
            } else {
                "http"
            },
            host: if url.host_str() == Some("localhost") {
                "localhost"
            } else {
                "127.0.0.1"
            },
            port: url.port_or_known_default().expect("fixture port"),
            maximum_bytes,
            maximum_redirects: MAX_DOWNLOAD_REDIRECTS,
        }
    }
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
        cache, download_file_with_progress, download_user_agent, DownloadCancellation,
        DownloadPolicy, InstallError,
    };
    use crate::models::DownloadState;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
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

    fn read_request(stream: &mut TcpStream) {
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
    }

    fn write_fixed_response(stream: &mut TcpStream, body: &[u8], declared_length: usize) {
        if write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {declared_length}\r\nETag: \"fixture-etag\"\r\nConnection: close\r\n\r\n"
        )
        .is_ok()
        {
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    }

    fn serve_once(body: Vec<u8>, declared_length: Option<usize>) -> FixtureServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            read_request(&mut stream);
            let length = declared_length.unwrap_or(body.len());
            write_fixed_response(&mut stream, &body, length);
        });
        FixtureServer {
            url: format!("http://{address}/Mendix-11.12.2-Setup.exe"),
            handle,
        }
    }

    fn serve_chunked(body: Vec<u8>) -> FixtureServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            read_request(&mut stream);
            if write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .is_err()
            {
                return;
            }
            for chunk in body.chunks(16 * 1024) {
                if write!(stream, "{:x}\r\n", chunk.len()).is_err()
                    || stream.write_all(chunk).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n");
            let _ = stream.flush();
        });
        FixtureServer {
            url: format!("http://{address}/chunked"),
            handle,
        }
    }

    fn serve_redirect(body: Vec<u8>, allowed_target: bool) -> FixtureServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture server address");
        let handle = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("accept redirect request");
            read_request(&mut first);
            let location = if allowed_target {
                "/Mendix-11.12.2-Setup.exe".to_string()
            } else {
                format!("http://localhost:{}/untrusted.exe", address.port())
            };
            write!(
                first,
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("write redirect response");
            first.flush().expect("flush redirect response");
            drop(first);
            if allowed_target {
                let (mut second, _) = listener.accept().expect("accept redirected request");
                read_request(&mut second);
                write_fixed_response(&mut second, &body, body.len());
            }
        });
        FixtureServer {
            url: format!("http://{address}/redirect"),
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
        destination: &cache::InstallerCache,
        maximum_bytes: u64,
        states: &mut Vec<DownloadState>,
    ) -> Result<(), InstallError> {
        download_file_with_progress(
            &DownloadCancellation::new(),
            "11.12.2",
            url,
            destination,
            DownloadPolicy::fixture(url, maximum_bytes),
            |state, _, _, _, _| states.push(state),
        )
        .await
        .map(|_| ())
    }

    fn assert_no_temporary_files(directory: &Path) {
        let temporary_names = std::fs::read_dir(directory)
            .expect("read cache directory")
            .map(|entry| {
                entry
                    .expect("cache entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.starts_with("mendimaru-"))
            .collect::<Vec<_>>();
        assert!(
            temporary_names.is_empty(),
            "temporary cache files remain: {temporary_names:?}"
        );
    }

    fn assert_no_partial_files(directory: &Path) {
        let partial_names = std::fs::read_dir(directory)
            .expect("read cache directory")
            .map(|entry| {
                entry
                    .expect("cache entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".partial") || name.ends_with(".partial.json"))
            .collect::<Vec<_>>();
        assert!(
            partial_names.is_empty(),
            "partial files remain: {partial_names:?}"
        );
    }

    enum ResumeStep {
        /// Send a truncated 200 response with the first `bytes` of the body.
        Truncated { bytes: usize },
        /// Answer a Range request with 206 and the remainder of the body.
        Partial,
        /// Answer with a complete 200 response (origin changed or no Range).
        Full,
    }

    type CapturedRequests = std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>;

    fn serve_resume_sequence(
        body: Vec<u8>,
        steps: Vec<ResumeStep>,
    ) -> (String, CapturedRequests, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind resume fixture");
        let address = listener.local_addr().expect("resume fixture address");
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = std::sync::Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            for step in steps {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .expect("set resume timeout");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).expect("read resume request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                captured
                    .lock()
                    .expect("capture resume request")
                    .push(request.clone());
                let total = body.len();
                match step {
                    ResumeStep::Truncated { bytes } => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {total}\r\nETag: \"resume-v1\"\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(&body[..bytes]);
                        let _ = stream.flush();
                    }
                    ResumeStep::Partial => {
                        let request_text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                        let start = request_text
                            .lines()
                            .find_map(|line| line.strip_prefix("range: bytes="))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        let remaining = &body[start..];
                        let end = total - 1;
                        let _ = write!(
                            stream,
                            "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nETag: \"resume-v1\"\r\nConnection: close\r\n\r\n",
                            remaining.len()
                        );
                        let _ = stream.write_all(remaining);
                        let _ = stream.flush();
                    }
                    ResumeStep::Full => {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {total}\r\nETag: \"resume-v2\"\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(&body);
                        let _ = stream.flush();
                    }
                }
            }
        });
        (
            format!("http://{address}/Mendix-11.12.2-Setup.exe"),
            requests,
            handle,
        )
    }

    #[test]
    fn production_url_policy_allows_only_exact_https_artifact_origins() {
        let policy = DownloadPolicy::production();
        assert!(policy
            .validate(
                &reqwest::Url::parse(
                    "https://artifacts.rnd.mendix.com/path/installer.exe?token=fixture"
                )
                .expect("allowed URL")
            )
            .is_ok());
        for url in [
            "http://artifacts.rnd.mendix.com/installer.exe",
            "https://evil.artifacts.rnd.mendix.com/installer.exe",
            "https://artifacts.rnd.mendix.com.evil.test/installer.exe",
            "https://artifacts.rnd.mendix.com:444/installer.exe",
            "https://user@artifacts.rnd.mendix.com/installer.exe",
        ] {
            assert!(
                policy
                    .validate(&reqwest::Url::parse(url).expect("test URL"))
                    .is_err(),
                "unexpectedly trusted {url}"
            );
        }
    }

    #[test]
    fn cache_names_reject_path_traversal_even_without_the_public_validator() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        assert!(
            cache::InstallerCache::open_for_tests(temporary.path(), r"11.12.2/../../evil").is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_shared_partial_symlink_is_ignored_by_the_private_cache() {
        use crate::app_paths::AppPaths;
        use std::os::unix::fs::symlink;
        use tokio::io::AsyncWriteExt;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let shared_cache = temporary.path().join("shared/.mendimaru/installers");
        std::fs::create_dir_all(&shared_cache).expect("legacy shared cache");
        let sentinel = temporary.path().join("sentinel");
        std::fs::write(&sentinel, b"unchanged").expect("sentinel");
        let legacy_partial = shared_cache.join("Mendix-11.12.2-Setup.exe.download");
        symlink(&sentinel, &legacy_partial).expect("legacy partial symlink");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("private-cache"),
        );
        let destination =
            cache::InstallerCache::open(&paths, "11.12.2").expect("host-private installer cache");
        let mut partial = destination.open_partial_payload().expect("private payload");

        partial
            .write_all(b"private bytes")
            .await
            .expect("write private payload");

        let partial_path = destination.partial_payload_name();
        assert!(paths
            .cache_directory()
            .join("installers")
            .join(partial_path)
            .starts_with(paths.cache_directory()));
        assert_eq!(
            std::fs::read(&sentinel).expect("read sentinel"),
            b"unchanged"
        );
        assert!(legacy_partial.is_symlink());
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
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let server = serve_once(pe_fixture(), None);
        let url = server.url.clone();
        let mut states = Vec::new();

        run_fixture_download(&url, &destination, u64::MAX, &mut states)
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
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn same_origin_redirect_is_followed_and_untrusted_host_redirect_is_blocked() {
        let allowed_root = tempfile::tempdir().expect("allowed cache directory");
        let allowed_cache = cache::InstallerCache::open_for_tests(allowed_root.path(), "11.12.2")
            .expect("allowed installer cache");
        let allowed = serve_redirect(pe_fixture(), true);
        let allowed_url = allowed.url.clone();
        run_fixture_download(&allowed_url, &allowed_cache, u64::MAX, &mut Vec::new())
            .await
            .expect("same-origin redirect");
        allowed.finish();
        assert!(matches!(
            cache::inspect(&allowed_cache, "11.12.2", &allowed_url).await,
            cache::CacheInspection::Valid(_)
        ));

        let blocked_root = tempfile::tempdir().expect("blocked cache directory");
        let blocked_cache = cache::InstallerCache::open_for_tests(blocked_root.path(), "11.12.2")
            .expect("blocked installer cache");
        let blocked = serve_redirect(pe_fixture(), false);
        let blocked_url = blocked.url.clone();
        let result =
            run_fixture_download(&blocked_url, &blocked_cache, u64::MAX, &mut Vec::new()).await;
        blocked.finish();
        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!blocked_cache
            .installer_path()
            .expect("installer path")
            .exists());
        assert_no_temporary_files(blocked_root.path());
    }

    #[tokio::test]
    async fn http_error_payload_never_replaces_the_installer_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let server = serve_once(vec![b'X'; 1024 * 1024 + 4096], None);
        let url = server.url.clone();
        let mut states = Vec::new();

        let result = run_fixture_download(&url, &destination, u64::MAX, &mut states).await;
        server.finish();

        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!destination
            .installer_path()
            .expect("installer path")
            .exists());
        assert!(!destination.metadata_path().expect("metadata path").exists());
        assert_no_temporary_files(temporary.path());
        assert!(!states.contains(&DownloadState::Downloaded));
    }

    #[tokio::test]
    async fn truncated_http_response_never_becomes_a_cache_entry() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let payload = pe_fixture();
        let server = serve_once(payload.clone(), Some(payload.len() + 8192));
        let url = server.url.clone();
        let mut states = Vec::new();

        let result = run_fixture_download(&url, &destination, u64::MAX, &mut states).await;
        server.finish();

        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!destination
            .installer_path()
            .expect("installer path")
            .exists());
        assert_no_temporary_files(temporary.path());
        assert!(!states.contains(&DownloadState::Downloaded));
    }

    #[tokio::test]
    async fn oversized_content_length_is_rejected_before_a_payload_is_created() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let maximum = 64 * 1024_u64;
        let server = serve_once(vec![0_u8; 1], Some(maximum as usize + 1));
        let url = server.url.clone();

        let result = run_fixture_download(&url, &destination, maximum, &mut Vec::new()).await;
        server.finish();

        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!destination
            .installer_path()
            .expect("installer path")
            .exists());
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn chunked_response_is_stopped_at_the_streaming_size_limit() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let maximum = 64 * 1024_u64;
        let server = serve_chunked(vec![0x5a; maximum as usize + 1]);
        let url = server.url.clone();

        let result = run_fixture_download(&url, &destination, maximum, &mut Vec::new()).await;
        server.finish();

        assert!(matches!(result, Err(InstallError::Other(_))));
        assert!(!destination
            .installer_path()
            .expect("installer path")
            .exists());
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn payload_exactly_at_the_streaming_limit_is_accepted() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let payload = pe_fixture();
        let maximum = payload.len() as u64;
        let server = serve_chunked(payload);
        let url = server.url.clone();

        run_fixture_download(&url, &destination, maximum, &mut Vec::new())
            .await
            .expect("boundary-sized payload");
        server.finish();
        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn cancelled_forced_download_preserves_the_previous_verified_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let original_server = serve_once(pe_fixture(), None);
        let original_url = original_server.url.clone();
        let mut original_states = Vec::new();
        run_fixture_download(&original_url, &destination, u64::MAX, &mut original_states)
            .await
            .expect("download original fixture");
        original_server.finish();

        let replacement_server = serve_once(pe_fixture(), None);
        let cancellation = DownloadCancellation::new();
        cancellation.cancel(false);
        let result = download_file_with_progress(
            &cancellation,
            "11.12.2",
            &replacement_server.url,
            &destination,
            DownloadPolicy::fixture(&replacement_server.url, u64::MAX),
            |_, _, _, _, _| {},
        )
        .await;
        replacement_server.finish();

        assert!(matches!(result, Err(InstallError::Cancelled(_))));
        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &original_url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn interrupted_download_resumes_from_the_verified_partial() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let body = pe_fixture();
        let split = body.len() / 3;
        let (url, requests, server) = serve_resume_sequence(
            body,
            vec![ResumeStep::Truncated { bytes: split }, ResumeStep::Partial],
        );

        let first = download_file_with_progress(
            &DownloadCancellation::new(),
            "11.12.2",
            &url,
            &destination,
            DownloadPolicy::fixture(&url, u64::MAX),
            |_, _, _, _, _| {},
        )
        .await;
        assert!(matches!(first, Err(InstallError::Other(_))));

        let partial_path = temporary.path().join("Mendix-11.12.2-Setup.exe.partial");
        assert_eq!(
            std::fs::metadata(&partial_path)
                .expect("partial payload")
                .len(),
            split as u64
        );

        let second = download_file_with_progress(
            &DownloadCancellation::new(),
            "11.12.2",
            &url,
            &destination,
            DownloadPolicy::fixture(&url, u64::MAX),
            |_, _, _, _, _| {},
        )
        .await;
        second.expect("resumed download completes");
        server.join().expect("resume fixture completes");

        let resume_request = {
            let captured = requests.lock().expect("captured requests");
            assert_eq!(captured.len(), 2);
            String::from_utf8_lossy(&captured[1]).to_ascii_lowercase()
        };
        assert!(resume_request.contains(&format!("range: bytes={split}-")));
        assert!(resume_request.contains("if-range: \"resume-v1\""));
        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert_no_partial_files(temporary.path());
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn changed_installer_restarts_the_download_from_the_beginning() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let body = pe_fixture();
        let split = body.len() / 2;
        let (url, requests, server) = serve_resume_sequence(
            body,
            vec![ResumeStep::Truncated { bytes: split }, ResumeStep::Full],
        );

        let first = download_file_with_progress(
            &DownloadCancellation::new(),
            "11.12.2",
            &url,
            &destination,
            DownloadPolicy::fixture(&url, u64::MAX),
            |_, _, _, _, _| {},
        )
        .await;
        assert!(matches!(first, Err(InstallError::Other(_))));

        download_file_with_progress(
            &DownloadCancellation::new(),
            "11.12.2",
            &url,
            &destination,
            DownloadPolicy::fixture(&url, u64::MAX),
            |_, _, _, _, _| {},
        )
        .await
        .expect("full restart completes");
        server.join().expect("restart fixture completes");

        {
            let captured = requests.lock().expect("captured requests");
            assert_eq!(captured.len(), 2);
        }
        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert_no_partial_files(temporary.path());
    }

    #[tokio::test]
    async fn failed_forced_download_preserves_the_previous_verified_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let original = serve_once(pe_fixture(), None);
        let original_url = original.url.clone();
        run_fixture_download(&original_url, &destination, u64::MAX, &mut Vec::new())
            .await
            .expect("original download");
        original.finish();

        let invalid = serve_once(vec![b'X'; 1024 * 1024 + 4096], None);
        let invalid_url = invalid.url.clone();
        assert!(
            run_fixture_download(&invalid_url, &destination, u64::MAX, &mut Vec::new())
                .await
                .is_err()
        );
        invalid.finish();

        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &original_url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert_no_temporary_files(temporary.path());
    }

    #[tokio::test]
    async fn successful_forced_download_atomically_replaces_the_previous_cache() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let destination = cache::InstallerCache::open_for_tests(temporary.path(), "11.12.2")
            .expect("installer cache");
        let original = serve_once(pe_fixture(), None);
        let original_url = original.url.clone();
        run_fixture_download(&original_url, &destination, u64::MAX, &mut Vec::new())
            .await
            .expect("original download");
        original.finish();

        let mut replacement_payload = pe_fixture();
        replacement_payload[4096] = 0xa5;
        let replacement = serve_once(replacement_payload, None);
        let replacement_url = replacement.url.clone();
        run_fixture_download(&replacement_url, &destination, u64::MAX, &mut Vec::new())
            .await
            .expect("replacement download");
        replacement.finish();

        assert!(matches!(
            cache::inspect(&destination, "11.12.2", &replacement_url).await,
            cache::CacheInspection::Valid(_)
        ));
        assert_eq!(
            cache::inspect(&destination, "11.12.2", &original_url).await,
            cache::CacheInspection::Invalid(cache::CacheValidationError::SourceMismatch)
        );
        assert_no_temporary_files(temporary.path());
    }
}
