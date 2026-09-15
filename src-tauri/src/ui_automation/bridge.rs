use super::*;
use crate::contracts::{ArtifactDescriptor, ArtifactKind};
use crate::winboat::security::{
    authenticate_bounded_report, authenticated_envelope, OperationSecurity,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn channel_path(base: &Path, suffix: &str) -> PathBuf {
    let mut path = base.as_os_str().to_owned();
    path.push(suffix);
    path.into()
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| ())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ())
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, ()> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let metadata = file.metadata().map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > MAX_RESPONSE || metadata.nlink() != 1 {
        return Err(());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err(());
    }
    Ok(Some(bytes))
}

/// Cancellation uses a separate signed file and request ID. Removing a queued
/// request cannot authorize a later action; the guest also checks its deadline.
struct Pending {
    request: PathBuf,
    report: PathBuf,
    cancel: PathBuf,
    cancellation: Vec<u8>,
    complete: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.complete {
            let _ = write_new(&self.cancel, &self.cancellation);
        }
        let _ = std::fs::remove_file(&self.request);
        let _ = std::fs::remove_file(&self.report);
        if self.complete {
            let _ = std::fs::remove_file(&self.cancel);
        }
    }
}

pub(crate) async fn request(
    request: &Request,
    cancellation: Option<&crate::process::CancellationToken>,
) -> Result<Value, BackendError> {
    request.validate()?;
    let op = request.operation;
    if cancellation.is_some_and(|token| token.is_cancelled()) {
        return Err(error(op, "ui-cancelled"));
    }
    let (base, security, sequence) = crate::winboat::ui_channel(&request.session_id)
        .map_err(|_| error(op, "ui-session-unavailable"))?;
    let id = crate::contracts::secure_identifier("ui")?;
    let req_path = channel_path(&base, ".ui.request");
    let report_path = channel_path(&base, ".ui.report");
    let cancel_path = channel_path(&base, ".ui.cancel");
    // This fixed mailbox is owned by the serialized keeper. Old reports may
    // remain after caller death; their unpredictable ID can never satisfy this request.
    for path in [&report_path, &cancel_path] {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
                return Err(error(op, "ui-bridge-untrusted"));
            }
            std::fs::remove_file(path).map_err(|_| error(op, "ui-bridge-untrusted"))?;
        }
    }
    let deadline = chrono::Utc::now().timestamp_millis() + request.timeout_ms as i64;
    let payload = serde_json::to_vec(&json!({"id":id,"expiresAt":deadline,"request":request}))
        .map_err(|_| error(op, "ui-invalid-request"))?;
    let envelope = authenticated_envelope(&security, sequence, &payload)
        .map_err(|_| error(op, "ui-invalid-request"))?;
    let cancellation_payload = authenticated_envelope(
        &security,
        sequence,
        &serde_json::to_vec(&json!({"id":id,"cancel":true})).unwrap(),
    )
    .map_err(|_| error(op, "ui-invalid-request"))?;
    write_new(&req_path, &envelope).map_err(|_| error(op, "ui-bridge-untrusted"))?;
    let mut pending = Pending {
        request: req_path,
        report: report_path,
        cancel: cancel_path,
        cancellation: cancellation_payload,
        complete: false,
    };
    let until = tokio::time::Instant::now() + Duration::from_millis(request.timeout_ms + 3000);
    let mut cancellation_sent = false;
    loop {
        if !cancellation_sent && cancellation.is_some_and(|token| token.is_cancelled()) {
            write_new(&pending.cancel, &pending.cancellation)
                .map_err(|_| error(op, "ui-bridge-untrusted"))?;
            cancellation_sent = true;
        }
        if let Some(bytes) =
            read_bounded(&pending.report).map_err(|_| error(op, "ui-bridge-untrusted"))?
        {
            let response = match decode(&bytes, &security, &id, op) {
                Ok(value) => value,
                Err(error) => {
                    pending.complete = error.message != "ui-bridge-untrusted";
                    return Err(error);
                }
            };
            if let Some(response) = response {
                pending.complete = true;
                return if request.operation == Operation::Screenshot {
                    persist_screenshot(&request.session_id, response)
                } else {
                    Ok(response)
                };
            }
        }
        if tokio::time::Instant::now() >= until {
            return Err(error(op, "ui-helper-timeout"));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn decode(
    bytes: &[u8],
    security: &OperationSecurity,
    id: &str,
    op: Operation,
) -> Result<Option<Value>, BackendError> {
    let authenticated =
        authenticate_bounded_report(bytes, security, MAX_RESPONSE, 12 * 1024 * 1024)
            .map_err(|_| error(op, "ui-bridge-untrusted"))?;
    let value: Value = serde_json::from_slice(&authenticated.payload)
        .map_err(|_| error(op, "ui-bridge-untrusted"))?;
    // Authentication is not freshness: a prior signed reply is ignored.
    if value["id"] != id {
        return Ok(None);
    }
    if value["ok"] == true {
        return Ok(Some(value["data"].clone()));
    }
    Err(error(
        op,
        value["reason"].as_str().unwrap_or("ui-provider-failed"),
    ))
}

fn persist_screenshot(session_id: &str, response: Value) -> Result<Value, BackendError> {
    let failure = || error(Operation::Screenshot, "ui-capture-failed");
    let bytes = STANDARD
        .decode(response["png"].as_str().ok_or_else(failure)?)
        .map_err(|_| failure())?;
    if bytes.len() > 8 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(failure());
    }
    let paths = crate::app_paths::AppPaths::discover_for_cli().map_err(|_| failure())?;
    paths.ensure_cache_directory().map_err(|_| failure())?;
    // Each artifact gets a new private directory. No caller-controlled file path.
    let directory = tempfile::Builder::new()
        .prefix("ui-artifact-")
        .tempdir_in(paths.cache_directory())
        .map_err(|_| failure())?;
    let path = directory.path().join("window.png");
    write_new(&path, &bytes).map_err(|_| failure())?;
    File::open(directory.path())
        .and_then(|f| f.sync_all())
        .map_err(|_| failure())?;
    let mut artifact = ArtifactDescriptor::create(
        session_id,
        BackendId::LinuxWinboat,
        ArtifactKind::Screenshot,
    )?;
    artifact.media_type = Some("image/png".into());
    artifact.location = Some(path.to_string_lossy().into_owned());
    artifact.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    artifact.size_bytes = Some(bytes.len() as u64);
    artifact.backend_diagnostic_ref =
        Some("PrintWindow; physical window pixels; visual verification required".into());
    let _ = directory.keep();
    serde_json::to_value(artifact).map_err(|_| failure())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticates_responses_without_accepting_stale_or_tampered_results() {
        let security = OperationSecurity::fixture();
        let envelope = authenticated_envelope(
            &security,
            1,
            br#"{"id":"ui_expected","ok":true,"data":{"sessionId":"selected"}}"#,
        )
        .unwrap();
        assert!(decode(&envelope, &security, "ui_other", Operation::Tree)
            .unwrap()
            .is_none());
        assert_eq!(
            decode(&envelope, &security, "ui_expected", Operation::Tree)
                .unwrap()
                .unwrap()["sessionId"],
            "selected"
        );
        let mut tampered: Value = serde_json::from_slice(&envelope).unwrap();
        tampered["mac"] = Value::String("0".repeat(64));
        assert_eq!(
            decode(
                &serde_json::to_vec(&tampered).unwrap(),
                &security,
                "ui_expected",
                Operation::Tree
            )
            .unwrap_err()
            .message,
            "ui-bridge-untrusted"
        );
        let failure = authenticated_envelope(
            &security,
            2,
            br#"{"id":"ui_expected","ok":false,"reason":"private-secret"}"#,
        )
        .unwrap();
        assert_eq!(
            decode(&failure, &security, "ui_expected", Operation::Tree)
                .unwrap_err()
                .message,
            "ui-provider-failed"
        );
    }
    #[test]
    fn rejects_unbounded_and_indirect_mailboxes_and_never_overwrites_requests() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report");
        write_new(&path, b"original").unwrap();
        assert!(write_new(&path, b"replacement").is_err());
        assert_eq!(read_bounded(&path).unwrap().unwrap(), b"original");
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_bounded(&link).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&path, &link).unwrap();
        assert!(read_bounded(&path).is_err());
        std::fs::remove_file(&link).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_RESPONSE + 1)
            .unwrap();
        assert!(read_bounded(&path).is_err());
        let fifo = directory.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_bounded(&fifo).is_err());
    }
    #[test]
    fn cancellation_targets_one_authenticated_request_and_removes_queued_input() {
        let directory = tempfile::tempdir().unwrap();
        let security = OperationSecurity::fixture();
        let request = directory.path().join("request");
        write_new(&request, b"queued").unwrap();
        let cancel = directory.path().join("cancel");
        let cancellation =
            authenticated_envelope(&security, 4, br#"{"id":"ui_exact","cancel":true}"#).unwrap();
        drop(Pending {
            request: request.clone(),
            report: directory.path().join("report"),
            cancel: cancel.clone(),
            cancellation,
            complete: false,
        });
        assert!(!request.exists());
        let authenticated = authenticate_bounded_report(
            &std::fs::read(cancel).unwrap(),
            &security,
            MAX_RESPONSE,
            8192,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&authenticated.payload).unwrap()["id"],
            "ui_exact"
        );
    }
}
