//! Guest discovery over the retained monitor, without opening an RDP connection.
use super::operation::WindowsStudioSessionReport;
use super::security::{authenticate_report, authenticated_envelope, MAX_REPORT_BYTES};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

pub(super) async fn query(session: &str) -> Result<Vec<WindowsStudioSessionReport>, String> {
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _guard = SERIAL
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let failure = || {
        "the Studio monitor could not verify guest sessions; reconnect before retrying".to_string()
    };
    let (base, security, sequence) = super::monitor_channel(session).map_err(|_| failure())?;
    let path = |suffix: &str| {
        let mut p = base.as_os_str().to_owned();
        p.push(suffix);
        PathBuf::from(p)
    };
    let request = path(".sessions.request");
    let report = path(".sessions.report");
    for p in [&request, &report] {
        if let Ok(m) = std::fs::symlink_metadata(p) {
            if !m.is_file() || m.file_type().is_symlink() || m.nlink() != 1 {
                return Err(failure());
            }
            std::fs::remove_file(p).map_err(|_| failure())?;
        }
    }
    struct Files(PathBuf, PathBuf);
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(&self.1);
        }
    }
    let _files = Files(request.clone(), report.clone());
    let id = crate::contracts::secure_identifier("query").map_err(|_| failure())?;
    let payload = serde_json::to_vec(&serde_json::json!({"id":id,"sessionId":session,"expiresAt":chrono::Utc::now().timestamp_millis()+30000})).map_err(|_|failure())?;
    let bytes = authenticated_envelope(&security, sequence, &payload).map_err(|_| failure())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&request)
        .map_err(|_| failure())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| failure())?;
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < until {
        match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&report)
        {
            Ok(file) => {
                let m = file.metadata().map_err(|_| failure())?;
                if !m.is_file() || m.nlink() != 1 || m.len() > MAX_REPORT_BYTES {
                    return Err(failure());
                }
                let mut bytes = Vec::new();
                file.take(MAX_REPORT_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| failure())?;
                if bytes.len() as u64 > MAX_REPORT_BYTES {
                    return Err(failure());
                }
                let auth = authenticate_report(&bytes, &security).map_err(|_| failure())?;
                let value: serde_json::Value =
                    serde_json::from_slice(&auth.payload).map_err(|_| failure())?;
                if value["id"] != id || value["sessionId"] != session || value["ok"] != true {
                    return Err(failure());
                }
                return serde_json::from_value(value["sessions"].clone()).map_err(|_| failure());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(failure()),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(failure())
}
