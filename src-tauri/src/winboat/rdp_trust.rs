//! Port-independent trust for the WinBoat RDP server certificate (#227).
//!
//! FreeRDP keeps its trust-on-first-use pins per `host_port`. WinBoat maps the
//! guest RDP port from a host port range, and every container recreation (Run
//! Locally forwarding, its rollback, settings changes) can move it to another
//! port of that range. Left alone, every move turned the next connection into
//! a fresh TOFU decision, and an unrelated stale pin could linger on a port
//! that a later recreation hands out again.
//!
//! Mendimaru therefore owns one trust anchor per VM identity and treats the
//! FreeRDP per-port file only as a cache it rewrites before each connection:
//!
//! * **Anchor** – a confirmed certificate exists. FreeRDP may accept exactly
//!   its fingerprint, and the per-port pin is rewritten to the same
//!   certificate so no stale per-port entry can widen trust.
//! * **Legacy pin** – no anchor yet, but an older version pinned the current
//!   port (or every older pin agrees on one certificate). That certificate is
//!   used exactly like an anchor and is promoted after a confirmed operation.
//! * **First use** – nothing is known. FreeRDP's TOFU accepts the presented
//!   certificate and writes the per-port pin. The pin becomes the anchor only
//!   after Windows returned an HMAC-authenticated operation report through
//!   that connection; a pin created by an attempt that failed is removed, so
//!   no transient listener can ever become trusted.
//!
//! Every policy carries `deny`: FreeRDP must never block on its interactive
//! "Do you trust the above certificate?" prompt, which would otherwise hang
//! until the operation deadline.

use crate::models::AppConfig;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::io::Write;
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
        let body = pem
            .lines()
            .skip_while(|line| line.trim() != "-----BEGIN CERTIFICATE-----")
            .skip(1)
            .take_while(|line| line.trim() != "-----END CERTIFICATE-----")
            .map(str::trim)
            .collect::<String>();
        if body.is_empty() {
            return Err(crate::tr!(
                "error-freerdp-pin-invalid",
                error = "the certificate body is missing"
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
}

impl TrustAttempt {
    pub(super) fn prepare(config: &AppConfig, trust_store: &Path) -> Result<Self, String> {
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

    /// Promotes the certificate this attempt connected with to the VM anchor.
    /// Call only after an authenticated Windows report proved the connection
    /// reached the real guest.
    pub(super) fn confirm(&self) -> Result<(), String> {
        let certificate = match self.source {
            TrustSource::Anchor => return Ok(()),
            TrustSource::LegacyPin => self.certificate.clone(),
            TrustSource::FirstUse => read_pin(&self.port_pin_path).ok().flatten(),
        };
        let Some(certificate) = certificate else {
            return Err(crate::tr!(
                "error-freerdp-pin-invalid",
                error = "the accepted certificate was not recorded"
            ));
        };
        write_private_atomic(&self.anchor_path, certificate.to_pem().as_bytes())
            .map_err(|error| crate::tr!("error-freerdp-trust-directory-create", error = error))
    }

    /// Removes a pin that FreeRDP created through TOFU during an attempt that
    /// never produced an authenticated report. Pins that existed before the
    /// attempt (anchors and legacy pins) are never touched.
    pub(super) fn discard_unconfirmed(&self) {
        if self.source == TrustSource::FirstUse {
            let _ = std::fs::remove_file(&self.port_pin_path);
        }
    }
}

fn prepare_for(
    trust_store: &Path,
    identity: &str,
    host: &str,
    port: u16,
) -> Result<TrustAttempt, String> {
    let anchor_path = trust_store
        .join(ANCHOR_DIRECTORY)
        .join(format!("{identity}.pem"));
    let server_directory = trust_store.join(FREERDP_SERVER_DIRECTORY);
    // FreeRDP lowercases the whole `<host>_<port>.pem` name.
    let port_pin_path = server_directory.join(format!("{}_{port}.pem", host.to_ascii_lowercase()));

    if let Some(anchor) = read_pin(&anchor_path)? {
        sync_port_pin(&port_pin_path, &anchor)?;
        return Ok(TrustAttempt {
            source: TrustSource::Anchor,
            anchor_path,
            port_pin_path,
            certificate: Some(anchor),
        });
    }

    match read_pin(&port_pin_path) {
        Ok(Some(certificate)) => {
            return Ok(TrustAttempt {
                source: TrustSource::LegacyPin,
                anchor_path,
                port_pin_path,
                certificate: Some(certificate),
            });
        }
        Ok(None) => {}
        // A truncated or foreign per-port file carries no trust decision. It
        // must not block every future connection, and FreeRDP would treat it
        // as absent anyway, so it is cleared before the first-use path.
        Err(_) => {
            std::fs::remove_file(&port_pin_path)
                .map_err(|error| crate::tr!("error-freerdp-pin-read", error = error))?;
        }
    }

    if let Some(certificate) = agreed_legacy_pin(&server_directory, host) {
        sync_port_pin(&port_pin_path, &certificate)?;
        return Ok(TrustAttempt {
            source: TrustSource::LegacyPin,
            anchor_path,
            port_pin_path,
            certificate: Some(certificate),
        });
    }

    Ok(TrustAttempt {
        source: TrustSource::FirstUse,
        anchor_path,
        port_pin_path,
        certificate: None,
    })
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
/// Disagreeing pins prove that at least one of them is stale, so none of them
/// is trusted and the connection falls back to a confirmed first use.
fn agreed_legacy_pin(server_directory: &Path, host: &str) -> Option<PinnedCertificate> {
    let prefix = format!("{}_", host.to_ascii_lowercase());
    let mut agreed: Option<PinnedCertificate> = None;
    for entry in std::fs::read_dir(server_directory).ok()?.flatten() {
        let name = entry.file_name();
        let Some(port) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|rest| rest.strip_suffix(".pem"))
        else {
            continue;
        };
        if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(Some(certificate)) = read_pin(&entry.path()) else {
            continue;
        };
        match &agreed {
            Some(existing) if existing.fingerprint != certificate.fingerprint => return None,
            Some(_) => {}
            None => agreed = Some(certificate),
        }
    }
    agreed
}

fn read_pin(path: &Path) -> Result<Option<PinnedCertificate>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::tr!("error-freerdp-pin-read", error = error)),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_PIN_BYTES {
        return Err(crate::tr!(
            "error-freerdp-pin-invalid",
            error = "the pin is not a bounded regular file"
        ));
    }
    let pem =
        std::fs::read(path).map_err(|error| crate::tr!("error-freerdp-pin-read", error = error))?;
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
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
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
        let first = prepare(store.path(), 47300);
        assert_eq!(first.source(), TrustSource::FirstUse);
        assert_eq!(first.certificate_argument(), FIRST_USE_POLICY);

        // FreeRDP TOFU writes the per-port pin; Windows then authenticates.
        write_pin(store.path(), 47300, ALPHA);
        first.confirm().expect("confirm first use");
        assert!(anchor(store.path()).is_file());

        // A recreation moved the RDP port: no new TOFU decision is made.
        let moved = prepare(store.path(), 47304);
        assert_eq!(moved.source(), TrustSource::Anchor);
        assert_eq!(
            moved.certificate_argument(),
            format!("/cert:fingerprint:sha256:{ALPHA_FINGERPRINT},deny")
        );
        let synced = std::fs::read(pin_path(store.path(), 47304)).unwrap();
        assert_eq!(
            PinnedCertificate::from_pem(&synced).unwrap().fingerprint(),
            ALPHA_FINGERPRINT
        );
        moved.discard_unconfirmed();
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
        write_pin(store.path(), 47303, BETA);
        attempt.discard_unconfirmed();
        assert!(!pin_path(store.path(), 47303).exists());
        assert!(!anchor(store.path()).exists());

        let retry = prepare(store.path(), 47303);
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

        let attempt = prepare(store.path(), 47301);
        assert_eq!(attempt.source(), TrustSource::Anchor);
        let synced = std::fs::read(pin_path(store.path(), 47301)).unwrap();
        assert_eq!(
            PinnedCertificate::from_pem(&synced).unwrap().fingerprint(),
            ALPHA_FINGERPRINT
        );
    }

    #[test]
    fn legacy_pins_migrate_only_when_they_identify_one_certificate() {
        let store = tempfile::tempdir().unwrap();
        write_pin(store.path(), 47300, ALPHA);
        write_pin(store.path(), 47301, ALPHA);

        let agreed = prepare(store.path(), 47304);
        assert_eq!(agreed.source(), TrustSource::LegacyPin);
        assert!(agreed.certificate_argument().contains(ALPHA_FINGERPRINT));
        assert!(pin_path(store.path(), 47304).is_file());
        agreed.discard_unconfirmed();
        assert!(pin_path(store.path(), 47304).is_file());
        agreed.confirm().unwrap();
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
        assert_eq!(
            prepare(disagreeing.path(), 47309).source(),
            TrustSource::FirstUse
        );
        // The legacy pin of the exact port keeps the previous behaviour.
        assert_eq!(
            prepare(disagreeing.path(), 47301).source(),
            TrustSource::LegacyPin
        );
    }

    #[test]
    fn a_corrupt_per_port_pin_does_not_block_every_future_connection() {
        let store = tempfile::tempdir().unwrap();
        write_pin(store.path(), 47302, b"-----BEGIN CERTIFICATE-----\n");
        let attempt = prepare(store.path(), 47302);
        assert_eq!(attempt.source(), TrustSource::FirstUse);
        assert!(!pin_path(store.path(), 47302).exists());
    }

    #[test]
    fn a_corrupt_anchor_fails_closed() {
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store.path().join(ANCHOR_DIRECTORY)).unwrap();
        std::fs::write(anchor(store.path()), b"garbage").unwrap();
        assert!(prepare_for(store.path(), "winboat-test", "127.0.0.1", 47300).is_err());
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
