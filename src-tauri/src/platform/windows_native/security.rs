use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureReport {
    status: String,
    #[serde(default)]
    status_message: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    issuer: String,
}

pub(super) fn verify_mendix_executable(path: &Path) -> Result<String, String> {
    if !path.is_file()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(crate::tr!(
            "error-native-installer-invalid",
            path = path.display()
        ));
    }
    let before = sha256(path)?;
    let report = authenticode_report(path)?;
    validate_signature(&report)?;
    let after = sha256(path)?;
    if before != after {
        return Err(crate::tr!("error-native-installer-changed"));
    }
    Ok(before)
}

fn sha256(path: &Path) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|error| crate::tr!("error-native-installer-read", error = error))?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| crate::tr!("error-native-installer-read", error = error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn authenticode_report(path: &Path) -> Result<SignatureReport, String> {
    const SCRIPT: &str = r#"
$securityModule = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1'
Import-Module -Name $securityModule -Force -ErrorAction Stop
$signature = Get-AuthenticodeSignature -LiteralPath $env:MENDIMARU_SIGNATURE_PATH
$report = [ordered]@{
  status = $signature.Status.ToString()
  statusMessage = $signature.StatusMessage
  subject = if ($null -eq $signature.SignerCertificate) { '' } else { $signature.SignerCertificate.Subject }
  issuer = if ($null -eq $signature.SignerCertificate) { '' } else { $signature.SignerCertificate.Issuer }
}
$json = $report | ConvertTo-Json -Compress
$bytes = [Text.Encoding]::UTF8.GetBytes($json)
[Console]::Out.Write([Convert]::ToBase64String($bytes))
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "RemoteSigned",
            "-Command",
            SCRIPT,
        ])
        .env("MENDIMARU_SIGNATURE_PATH", path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| crate::tr!("error-native-signature-tool", error = error))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(crate::tr!(
            "error-native-signature-tool",
            error = if detail.is_empty() {
                output.status.to_string()
            } else {
                detail
            }
        ));
    }
    let encoded = String::from_utf8_lossy(&output.stdout);
    let json = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| crate::tr!("error-native-signature-parse", error = error))?;
    serde_json::from_slice::<SignatureReport>(&json)
        .map_err(|error| crate::tr!("error-native-signature-parse", error = error))
}

fn validate_signature(report: &SignatureReport) -> Result<(), String> {
    if !report.status.eq_ignore_ascii_case("valid") {
        let detail = if report.status_message.trim().is_empty() {
            report.status.as_str()
        } else {
            report.status_message.as_str()
        };
        return Err(crate::tr!(
            "error-native-signature-invalid",
            reason = detail
        ));
    }
    let trusted_publisher = report.subject.split(',').any(|component| {
        matches!(
            component.trim().to_ascii_lowercase().as_str(),
            "cn=mendix technology b.v."
                | "o=mendix technology b.v."
                | "cn=siemens ag"
                | "o=siemens ag"
        )
    });
    if !trusted_publisher {
        return Err(crate::tr!(
            "error-native-signature-publisher",
            publisher = if report.subject.is_empty() {
                crate::tr!("unknown-error")
            } else {
                report.subject.clone()
            }
        ));
    }
    let _trusted_signing_issuer = &report.issuer;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{sha256, validate_signature, verify_mendix_executable, SignatureReport};
    use std::fs;

    #[test]
    fn calculates_a_stable_sha256_digest() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let path = temporary.path().join("installer.exe");
        fs::write(&path, b"mendimaru").expect("write fixture");
        assert_eq!(
            sha256(&path).expect("digest"),
            "8edde51f9bc00d3fff19237df43bc1c6d058839aa1469b3fbd0c5479c929825d"
        );
    }

    #[test]
    fn accepts_only_valid_mendix_or_siemens_authenticode_publishers() {
        let valid = SignatureReport {
            status: "Valid".into(),
            status_message: "Signature verified.".into(),
            subject: "CN=Mendix Technology B.V., O=Mendix Technology B.V.".into(),
            issuer: "CN=Microsoft ID Verified CS AOC CA 03".into(),
        };
        assert!(validate_signature(&valid).is_ok());

        let siemens = SignatureReport {
            status: "Valid".into(),
            status_message: "Signature verified.".into(),
            subject: "CN=Siemens AG, O=Siemens AG, C=DE".into(),
            issuer: "CN=Microsoft ID Verified CS AOC CA 03".into(),
        };
        assert!(validate_signature(&siemens).is_ok());

        let mut unsigned = valid;
        unsigned.status = "NotSigned".into();
        assert!(validate_signature(&unsigned).is_err());

        let wrong_publisher = SignatureReport {
            status: "Valid".into(),
            status_message: String::new(),
            subject: "CN=Unrelated Publisher".into(),
            issuer: "CN=Trusted CA".into(),
        };
        assert!(validate_signature(&wrong_publisher).is_err());

        let deceptive_publisher = SignatureReport {
            status: "Valid".into(),
            status_message: String::new(),
            subject: "CN=Definitely Not Mendix Software LLC".into(),
            issuer: "CN=Trusted CA".into(),
        };
        assert!(validate_signature(&deceptive_publisher).is_err());
    }

    #[test]
    #[ignore = "hashes and verifies a locally installed signed Mendix executable"]
    fn live_verifies_a_mendix_authenticode_signature() {
        crate::i18n::initialize("en-US").expect("initialize localization");
        let path = std::env::var_os("MENDIMARU_LIVE_SIGNED_EXE")
            .map(std::path::PathBuf::from)
            .expect("set MENDIMARU_LIVE_SIGNED_EXE to a signed Mendix executable");
        let digest = verify_mendix_executable(&path).expect("trusted Mendix signature");
        assert_eq!(digest.len(), 64);
        assert!(digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }
}
