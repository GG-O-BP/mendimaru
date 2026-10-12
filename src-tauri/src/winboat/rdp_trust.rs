//! Port-independent trust for the WinBoat RDP server certificate (#227).
//!
//! FreeRDP keeps its trust-on-first-use pins per `host_port`. WinBoat maps the
//! guest RDP port from a host port range, and every container recreation (Run
//! Locally forwarding, its rollback, settings changes) can move it to another
//! port of that range. Left alone, every move turned the next connection into
//! a fresh TOFU decision, and an unrelated stale pin could linger on a port
//! that a later recreation hands out again.
//!
//! Mendimaru therefore owns one trust anchor per VM identity. Each connection
//! gets a private FreeRDP store; persistent per-port files are migration input
//! and a cache updated only after confirmation:
//!
//! * **Anchor** – a confirmed certificate exists. FreeRDP may accept exactly
//!   its fingerprint, and the private per-port pin is seeded with the same
//!   certificate so no stale per-port entry can widen trust.
//! * **Legacy pin** – no anchor yet, but an older version pinned the current
//!   port (or every older pin agrees on one certificate). That certificate is
//!   used exactly like an anchor and is promoted after a confirmed operation.
//! * **First use** – nothing is known. FreeRDP's TOFU writes only into an
//!   isolated, private attempt directory, never the persistent trust store.
//!   A pin is promoted after a valid authenticated Windows report. Failed,
//!   cancelled and abandoned attempts cannot become legacy trust on retry.
//!
//! TOFU is still an initial trust decision, not independent server
//! authentication: a report MAC protects the operation report, not the
//! identity of a server that received the bootstrap key over this connection.
//! Existing or conflicting trust must therefore never fall back to TOFU.
//!
//! Every policy carries `deny`: FreeRDP must never block on its interactive
//! "Do you trust the above certificate?" prompt, which would otherwise hang
//! until the operation deadline.

use super::operation::WindowsOperationFailure;
use crate::contracts::BackendErrorCode;
use crate::models::AppConfig;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(super) const FIRST_USE_POLICY: &str = "/cert:tofu,deny";
const ANCHOR_DIRECTORY: &str = "anchors";
const FREERDP_SERVER_DIRECTORY: &str = "freerdp/server";
const MAX_PIN_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrustSource {
    Anchor,
    LegacyPin,
    FirstUse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PinnedCertificate {
    der: Vec<u8>,
    fingerprint: String,
}

impl PinnedCertificate {
    pub(super) fn from_pem(pem: &[u8]) -> Result<Self, String> {
        let pem = std::str::from_utf8(pem)
            .map_err(|error| crate::tr!("error-freerdp-pin-invalid", error = error))?;
        let mut lines = pem.lines().map(str::trim);
        if !lines.any(|line| line == "-----BEGIN CERTIFICATE-----") {
            return Err(crate::tr!(
                "error-freerdp-pin-invalid",
                error = "the certificate header is missing"
            ));
        }
        let mut body = String::new();
        let mut ended = false;
        for line in lines {
            if line == "-----END CERTIFICATE-----" {
                ended = true;
                break;
            }
            body.push_str(line);
        }
        if !ended || body.is_empty() {
            return Err(crate::tr!(
                "error-freerdp-pin-invalid",
                error = "the certificate body or footer is missing"
            ));
        }
        let der = BASE64_STANDARD
            .decode(body)
            .map_err(|error| crate::tr!("error-freerdp-pin-invalid", error = error))?;
        let fingerprint = format!("{:x}", Sha256::digest(&der));
        Ok(Self { der, fingerprint })
    }

    #[cfg(test)]
    pub(super) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(super) fn policy(&self) -> String {
        format!("/cert:fingerprint:sha256:{},deny", self.fingerprint)
    }

    fn to_pem(&self) -> String {
        let encoded = BASE64_STANDARD.encode(&self.der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in encoded.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap_or_default());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        pem
    }
}

/// The trust decision for one FreeRDP connection attempt.
#[derive(Debug)]
pub(super) struct TrustAttempt {
    source: TrustSource,
    anchor_path: PathBuf,
    port_pin_path: PathBuf,
    certificate: Option<PinnedCertificate>,
    config_directory: tempfile::TempDir,
    attempt_pin_path: PathBuf,
    confirmed: bool,
}

impl TrustAttempt {
    #[cfg(test)]
    pub(super) fn fixture(trust_store: &Path) -> Self {
        prepare_for(trust_store, "winboat-test", "127.0.0.1", 47300).expect("fixture trust")
    }

    pub(super) fn prepare(
        config: &AppConfig,
        trust_store: &Path,
    ) -> Result<Self, WindowsOperationFailure> {
        prepare_for(
            trust_store,
            &anchor_identity(config),
            &config.rdp_host,
            config.rdp_port,
        )
    }

    #[cfg(test)]
    pub(super) fn source(&self) -> TrustSource {
        self.source
    }

    pub(super) fn certificate_argument(&self) -> String {
        self.certificate
            .as_ref()
            .map_or_else(|| FIRST_USE_POLICY.to_string(), PinnedCertificate::policy)
    }

    pub(super) fn config_directory(&self) -> &Path {
        self.config_directory.path()
    }

    /// Promotes this attempt's pin after a valid authenticated Windows report.
    /// Existing anchors are immutable, including a concurrent first-use
    /// attempt's anchor. Persistence errors must be returned to the caller.
    pub(super) fn confirm(&mut self) -> Result<(), WindowsOperationFailure> {
        if self.confirmed {
            return Ok(());
        }
        let certificate = match self.source {
            TrustSource::Anchor | TrustSource::LegacyPin => self.certificate.clone(),
            TrustSource::FirstUse => read_pin(&self.attempt_pin_path)?,
        };
        let Some(certificate) = certificate else {
            return Err(crate::tr!(
                "error-freerdp-pin-invalid",
                error = "the accepted certificate was not recorded"
            )
            .into());
        };
        if let Some(anchor) = read_pin(&self.anchor_path)? {
            if anchor != certificate {
                return Err(certificate_mismatch());
            }
        } else {
            match write_private_new(&self.anchor_path, certificate.to_pem().as_bytes()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if read_pin(&self.anchor_path)?.as_ref() != Some(&certificate) {
                        return Err(certificate_mismatch());
                    }
                }
                Err(error) => return Err(trust_io_error(error).into()),
            }
        }
        sync_port_pin(&self.port_pin_path, &certificate)?;
        self.confirmed = true;
        Ok(())
    }
}

fn prepare_for(
    trust_store: &Path,
    identity: &str,
    host: &str,
    port: u16,
) -> Result<TrustAttempt, WindowsOperationFailure> {
    private_directory(trust_store).map_err(trust_io_error)?;
    private_directory(&trust_store.join(ANCHOR_DIRECTORY)).map_err(trust_io_error)?;
    private_directory(&trust_store.join("freerdp")).map_err(trust_io_error)?;
    private_directory(&trust_store.join(FREERDP_SERVER_DIRECTORY)).map_err(trust_io_error)?;
    let anchor_path = trust_store
        .join(ANCHOR_DIRECTORY)
        .join(format!("{identity}.pem"));
    let server_directory = trust_store.join(FREERDP_SERVER_DIRECTORY);
    let port_pin_path = server_directory.join(format!("{}_{port}.pem", host.to_ascii_lowercase()));
    let (source, certificate) = if let Some(anchor) = read_pin(&anchor_path)? {
        (TrustSource::Anchor, Some(anchor))
    } else if let Some(pin) = read_pin(&port_pin_path)? {
        (TrustSource::LegacyPin, Some(pin))
    } else if let Some(pin) = agreed_legacy_pin(&server_directory, host)? {
        (TrustSource::LegacyPin, Some(pin))
    } else {
        (TrustSource::FirstUse, None)
    };

    // Only this attempt can read FreeRDP's TOFU output. Even a crash that
    // leaves this scratch directory behind cannot make it a legacy pin.
    let config_directory = tempfile::Builder::new()
        .prefix(".rdp-attempt-")
        .tempdir_in(trust_store)
        .map_err(trust_io_error)?;
    private_directory(&config_directory.path().join("freerdp")).map_err(trust_io_error)?;
    let attempt_server = config_directory.path().join(FREERDP_SERVER_DIRECTORY);
    private_directory(&attempt_server).map_err(trust_io_error)?;
    let attempt_pin_path = attempt_server.join(
        port_pin_path
            .file_name()
            .expect("the generated pin path has a filename"),
    );
    if let Some(pin) = &certificate {
        sync_port_pin(&attempt_pin_path, pin)?;
    }
    Ok(TrustAttempt {
        source,
        anchor_path,
        port_pin_path,
        certificate,
        config_directory,
        attempt_pin_path,
        confirmed: false,
    })
}

fn certificate_mismatch() -> WindowsOperationFailure {
    WindowsOperationFailure {
        message: crate::tr!("error-freerdp-certificate-mismatch"),
        exit_code: None,
        retryable: false,
        failure_kind: None,
        backend_error_code: Some(BackendErrorCode::RemoteDesktopCertificateMismatch),
    }
}

fn trust_io_error(error: std::io::Error) -> String {
    crate::tr!("error-freerdp-trust-directory-create", error = error)
}

fn private_directory(path: &Path) -> std::io::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::other(
            "the trust directory is not a direct directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid has no preconditions.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(std::io::Error::other(
                "the trust directory is not owned by this user",
            ));
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Stable, path-safe identity of the VM whose RDP endpoint is trusted. The
/// host port is deliberately excluded: it changes with every recreation.
pub(super) fn anchor_identity(config: &AppConfig) -> String {
    let digest = Sha256::digest(
        format!(
            "mendimaru-winboat-rdp-anchor\0{}\0{}\0{}",
            config.container_runtime.as_str(),
            config.container_name,
            config.rdp_host.to_ascii_lowercase()
        )
        .as_bytes(),
    );
    format!("winboat-{}", &format!("{digest:x}")[..32])
}

/// The single certificate every legacy per-port pin of this host agrees on.
/// Ambiguous or invalid prior trust must not be replaced by a new TOFU
/// decision. Resetting it is an explicit owner action.
fn agreed_legacy_pin(
    server_directory: &Path,
    host: &str,
) -> Result<Option<PinnedCertificate>, WindowsOperationFailure> {
    let prefix = format!("{}_", host.to_ascii_lowercase());
    let mut agreed: Option<PinnedCertificate> = None;
    for entry in std::fs::read_dir(server_directory).map_err(trust_io_error)? {
        let entry = entry.map_err(trust_io_error)?;
        let name = entry.file_name();
        let Some(port) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(".pem"))
        else {
            continue;
        };
        if !port.parse::<u16>().is_ok_and(|port| port != 0) {
            continue;
        }
        let Some(certificate) = read_pin(&entry.path())? else {
            continue;
        };
        match &agreed {
            Some(existing) if existing.fingerprint != certificate.fingerprint => {
                return Err(certificate_mismatch())
            }
            Some(_) => {}
            None => agreed = Some(certificate),
        }
    }
    Ok(agreed)
}

fn read_pin(path: &Path) -> Result<Option<PinnedCertificate>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Refuse symlinks at open time and do not block on a racing FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(crate::tr!(
            "error-freerdp-pin-invalid",
            error = "the pin is a symlink"
        ));
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::tr!("error-freerdp-pin-read", error = error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| crate::tr!("error-freerdp-pin-read", error = error))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_PIN_BYTES {
        return Err(crate::tr!(
            "error-freerdp-pin-invalid",
            error = "the pin is not a bounded regular file"
        ));
    }
    let mut pem = Vec::new();
    file.take(MAX_PIN_BYTES + 1)
        .read_to_end(&mut pem)
        .map_err(|error| crate::tr!("error-freerdp-pin-read", error = error))?;
    if pem.len() as u64 > MAX_PIN_BYTES {
        return Err(crate::tr!(
            "error-freerdp-pin-invalid",
            error = "the pin is too large"
        ));
    }
    PinnedCertificate::from_pem(&pem).map(Some)
}

fn sync_port_pin(path: &Path, certificate: &PinnedCertificate) -> Result<(), String> {
    if read_pin(path)
        .ok()
        .flatten()
        .is_some_and(|current| current == *certificate)
    {
        return Ok(());
    }
    write_private_atomic(path, certificate.to_pem().as_bytes())
        .map_err(|error| crate::tr!("error-freerdp-trust-directory-create", error = error))
}

fn write_private_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the pin path has no parent directory"))?;
    private_directory(directory)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".mendimaru-pin-")
        .tempfile_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn write_private_new(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no pin parent"))?;
    private_directory(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContainerRuntime;

    const ALPHA: &[u8] = b"-----BEGIN CERTIFICATE-----\nbWVuZGltYXJ1\n-----END CERTIFICATE-----\n";
    const BETA: &[u8] = b"-----BEGIN CERTIFICATE-----\nd2luYm9hdA==\n-----END CERTIFICATE-----\n";
    const ALPHA_FINGERPRINT: &str =
        "8edde51f9bc00d3fff19237df43bc1c6d058839aa1469b3fbd0c5479c929825d";

    fn pin_path(store: &Path, port: u16) -> PathBuf {
        store
            .join(FREERDP_SERVER_DIRECTORY)
            .join(format!("127.0.0.1_{port}.pem"))
    }

    fn write_pin(store: &Path, port: u16, pem: &[u8]) {
        let path = pin_path(store, port);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, pem).unwrap();
    }

    fn anchor(store: &Path) -> PathBuf {
        store.join(ANCHOR_DIRECTORY).join("winboat-test.pem")
    }

    fn prepare(store: &Path, port: u16) -> TrustAttempt {
        prepare_for(store, "winboat-test", "127.0.0.1", port).expect("prepare trust")
    }

    fn config() -> AppConfig {
        AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "missing-winboat".into(),
            compose_file: "missing-compose.yml".into(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:9".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: "/tmp/mendimaru-fixture".into(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "missing-freerdp".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        }
    }

    #[test]
    fn converts_a_pin_to_an_exact_non_interactive_fingerprint_policy() {
        let certificate = PinnedCertificate::from_pem(ALPHA).expect("fixture pin parses");
        assert_eq!(certificate.fingerprint(), ALPHA_FINGERPRINT);
        assert_eq!(
            certificate.policy(),
            format!("/cert:fingerprint:sha256:{ALPHA_FINGERPRINT},deny")
        );
        assert_eq!(FIRST_USE_POLICY, "/cert:tofu,deny");
        assert!(PinnedCertificate::from_pem(b"not a certificate").is_err());
        let round_trip = PinnedCertificate::from_pem(certificate.to_pem().as_bytes()).unwrap();
        assert_eq!(round_trip, certificate);
    }

    #[test]
    fn first_use_is_promoted_only_after_confirmation_and_survives_a_port_move() {
        let store = tempfile::tempdir().unwrap();
        let mut first = prepare(store.path(), 47300);
        assert_eq!(first.source(), TrustSource::FirstUse);
        assert_eq!(first.certificate_argument(), FIRST_USE_POLICY);

        // FreeRDP TOFU writes the per-port pin; Windows then authenticates.
        std::fs::write(&first.attempt_pin_path, ALPHA).unwrap();
        assert!(!pin_path(store.path(), 47300).exists());
        first.confirm().expect("confirm first use");
        assert!(anchor(store.path()).is_file());

        // A recreation moved the RDP port: no new TOFU decision is made.
        let mut moved = prepare(store.path(), 47304);
        assert_eq!(moved.source(), TrustSource::Anchor);
        assert_eq!(
            moved.certificate_argument(),
            format!("/cert:fingerprint:sha256:{ALPHA_FINGERPRINT},deny")
        );
        let synced = std::fs::read(&moved.attempt_pin_path).unwrap();
        assert_eq!(
            PinnedCertificate::from_pem(&synced).unwrap().fingerprint(),
            ALPHA_FINGERPRINT
        );
        moved.confirm().unwrap();
        drop(moved);
        assert!(
            pin_path(store.path(), 47304).is_file(),
            "anchored pins stay"
        );
    }

    #[test]
    fn a_failed_first_use_never_leaves_trust_behind() {
        let store = tempfile::tempdir().unwrap();
        let attempt = prepare(store.path(), 47303);
        // A transient listener answered and TOFU recorded it, but no
        // authenticated report ever arrived.
        std::fs::write(&attempt.attempt_pin_path, BETA).unwrap();
        let scratch = attempt.config_directory().to_path_buf();
        drop(attempt);
        assert!(!scratch.exists());
        assert!(!pin_path(store.path(), 47303).exists());
        assert!(!anchor(store.path()).exists());

        let mut retry = prepare(store.path(), 47303);
        assert_eq!(retry.source(), TrustSource::FirstUse);
        assert!(retry.confirm().is_err(), "nothing was accepted yet");
        assert!(!anchor(store.path()).exists());
    }

    #[test]
    fn stale_per_port_pins_are_overwritten_by_the_anchor() {
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store.path().join(ANCHOR_DIRECTORY)).unwrap();
        std::fs::write(anchor(store.path()), ALPHA).unwrap();
        // A port handed out again still carries an unrelated old pin, which
        // FreeRDP would otherwise accept as a known-hosts match.
        write_pin(store.path(), 47301, BETA);

        let mut attempt = prepare(store.path(), 47301);
        assert_eq!(attempt.source(), TrustSource::Anchor);
        let synced = std::fs::read(&attempt.attempt_pin_path).unwrap();
        assert_eq!(
            PinnedCertificate::from_pem(&synced).unwrap().fingerprint(),
            ALPHA_FINGERPRINT
        );
        // Preparing a connection does not mutate persistent prior trust.
        assert_eq!(std::fs::read(pin_path(store.path(), 47301)).unwrap(), BETA);
        attempt.confirm().unwrap();
        assert_eq!(
            read_pin(&pin_path(store.path(), 47301))
                .unwrap()
                .unwrap()
                .fingerprint(),
            ALPHA_FINGERPRINT
        );
    }

    #[test]
    fn legacy_pins_migrate_only_when_they_identify_one_certificate() {
        let store = tempfile::tempdir().unwrap();
        write_pin(store.path(), 47300, ALPHA);
        write_pin(store.path(), 47301, ALPHA);

        let mut agreed = prepare(store.path(), 47304);
        assert_eq!(agreed.source(), TrustSource::LegacyPin);
        assert!(agreed.certificate_argument().contains(ALPHA_FINGERPRINT));
        assert!(!pin_path(store.path(), 47304).exists());
        assert!(agreed.attempt_pin_path.is_file());
        agreed.confirm().unwrap();
        assert!(pin_path(store.path(), 47304).is_file());
        assert_eq!(
            read_pin(&anchor(store.path()))
                .unwrap()
                .unwrap()
                .fingerprint(),
            ALPHA_FINGERPRINT
        );

        let disagreeing = tempfile::tempdir().unwrap();
        write_pin(disagreeing.path(), 47300, ALPHA);
        write_pin(disagreeing.path(), 47301, BETA);
        let error =
            prepare_for(disagreeing.path(), "winboat-test", "127.0.0.1", 47309).unwrap_err();
        assert_eq!(
            error.backend_error_code,
            Some(BackendErrorCode::RemoteDesktopCertificateMismatch)
        );
        assert!(!error.retryable);
        assert!(!pin_path(disagreeing.path(), 47309).exists());
        // The legacy pin of the exact port keeps the previous behaviour.
        assert_eq!(
            prepare(disagreeing.path(), 47301).source(),
            TrustSource::LegacyPin
        );
    }

    #[test]
    fn a_corrupt_per_port_pin_never_silently_resets_prior_trust() {
        let store = tempfile::tempdir().unwrap();
        let truncated = b"-----BEGIN CERTIFICATE-----\n";
        write_pin(store.path(), 47302, truncated);
        assert!(prepare_for(store.path(), "winboat-test", "127.0.0.1", 47302).is_err());
        assert_eq!(
            std::fs::read(pin_path(store.path(), 47302)).unwrap(),
            truncated
        );
        // A confirmed anchor can repair an untrusted cache; without one an
        // invalid legacy pin requires an explicit owner decision.
        std::fs::write(anchor(store.path()), ALPHA).unwrap();
        let mut attempt = prepare(store.path(), 47302);
        assert_eq!(attempt.source(), TrustSource::Anchor);
        attempt.confirm().unwrap();
        assert_eq!(
            read_pin(&pin_path(store.path(), 47302))
                .unwrap()
                .unwrap()
                .fingerprint(),
            ALPHA_FINGERPRINT
        );
    }

    #[test]
    fn a_corrupt_anchor_fails_closed() {
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store.path().join(ANCHOR_DIRECTORY)).unwrap();
        std::fs::write(anchor(store.path()), b"garbage").unwrap();
        assert!(prepare_for(store.path(), "winboat-test", "127.0.0.1", 47300).is_err());
    }

    #[test]
    fn a_crashed_attempt_cannot_become_legacy_trust_on_retry() {
        let store = tempfile::tempdir().unwrap();
        let attempt = prepare(store.path(), 47300);
        std::fs::write(&attempt.attempt_pin_path, BETA).unwrap();
        // An abrupt process exit leaves a scratch directory, but it is not
        // part of the persistent FreeRDP pin namespace.
        let orphan = attempt.config_directory.keep();
        let retry = prepare(store.path(), 47300);
        assert_eq!(retry.source(), TrustSource::FirstUse);
        assert!(!pin_path(store.path(), 47300).exists());
        assert!(!anchor(store.path()).exists());
        std::fs::remove_dir_all(orphan).unwrap();
    }

    #[test]
    fn concurrent_first_uses_never_replace_an_existing_anchor() {
        let store = tempfile::tempdir().unwrap();
        let mut first = prepare(store.path(), 47300);
        let mut second = prepare(store.path(), 47301);
        std::fs::write(&first.attempt_pin_path, ALPHA).unwrap();
        std::fs::write(&second.attempt_pin_path, BETA).unwrap();
        first.confirm().unwrap();
        let error = second.confirm().unwrap_err();
        assert_eq!(
            error.backend_error_code,
            Some(BackendErrorCode::RemoteDesktopCertificateMismatch)
        );
        assert!(!error.retryable);
        assert_eq!(
            read_pin(&anchor(store.path()))
                .unwrap()
                .unwrap()
                .fingerprint(),
            ALPHA_FINGERPRINT
        );
        assert!(!pin_path(store.path(), 47301).exists());
    }

    #[test]
    fn simultaneous_matching_confirmations_are_idempotent() {
        let store = tempfile::tempdir().unwrap();
        let attempts = [
            TrustAttempt::fixture(store.path()),
            prepare(store.path(), 47301),
        ];
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let children: Vec<_> = attempts
            .into_iter()
            .map(|mut attempt| {
                std::fs::write(&attempt.attempt_pin_path, ALPHA).unwrap();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    attempt.confirm().expect("same first-use certificate");
                    attempt.confirm().expect("confirmation is idempotent");
                })
            })
            .collect();
        for child in children {
            child.join().unwrap();
        }
        assert_eq!(
            read_pin(&anchor(store.path()))
                .unwrap()
                .unwrap()
                .fingerprint(),
            ALPHA_FINGERPRINT
        );
    }

    #[test]
    fn confirmation_io_errors_do_not_establish_trust() {
        let store = tempfile::tempdir().unwrap();
        let mut attempt = prepare(store.path(), 47300);
        std::fs::write(&attempt.attempt_pin_path, ALPHA).unwrap();
        let anchors = store.path().join(ANCHOR_DIRECTORY);
        std::fs::remove_dir(&anchors).unwrap();
        std::fs::write(&anchors, b"not a directory").unwrap();
        assert!(attempt.confirm().is_err());
        assert!(!pin_path(store.path(), 47300).exists());
    }

    #[test]
    fn malformed_pem_never_becomes_a_fingerprint_pin() {
        for pem in [
            b"-----BEGIN CERTIFICATE-----\nbWVuZGltYXJ1\n".as_slice(),
            b"-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n",
            b"-----BEGIN CERTIFICATE-----\n???\n-----END CERTIFICATE-----\n",
            b"-----BEGIN PUBLIC KEY-----\nbWVuZGltYXJ1\n-----END PUBLIC KEY-----\n",
            b"\xff",
        ] {
            assert!(PinnedCertificate::from_pem(pem).is_err(), "{pem:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn untrusted_file_types_and_directory_symlinks_fail_closed() {
        use std::os::unix::fs::symlink;
        let store = tempfile::tempdir().unwrap();
        let target = store.path().join("outside.pem");
        std::fs::write(&target, ALPHA).unwrap();
        let prepared = prepare(store.path(), 47300);
        drop(prepared);
        symlink(&target, anchor(store.path())).unwrap();
        assert!(prepare_for(store.path(), "winboat-test", "127.0.0.1", 47300).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), ALPHA);

        let other = tempfile::tempdir().unwrap();
        symlink(
            store.path().join(ANCHOR_DIRECTORY),
            other.path().join(ANCHOR_DIRECTORY),
        )
        .unwrap();
        assert!(prepare_for(other.path(), "winboat-test", "127.0.0.1", 47300).is_err());
        let oversized = store.path().join("oversized.pem");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_PIN_BYTES + 1)
            .unwrap();
        assert!(read_pin(&oversized).is_err());
        assert!(read_pin(store.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn anchors_are_private_files() {
        use std::os::unix::fs::PermissionsExt;
        let store = tempfile::tempdir().unwrap();
        write_pin(store.path(), 47300, ALPHA);
        prepare(store.path(), 47300).confirm().unwrap();
        let mode = std::fs::metadata(anchor(store.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let directory = std::fs::metadata(store.path().join(ANCHOR_DIRECTORY))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(directory & 0o777, 0o700);
    }

    #[test]
    fn the_anchor_identity_ignores_the_moving_host_port() {
        let mut config = config();
        let first = anchor_identity(&config);
        config.rdp_port = 47304;
        assert_eq!(anchor_identity(&config), first);
        config.container_name = "OtherVM".into();
        assert_ne!(anchor_identity(&config), first);
        assert!(first.starts_with("winboat-"));
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
    }
}
