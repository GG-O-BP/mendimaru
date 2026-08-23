use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::fmt;

use super::scripts::powershell_literal;

const ENVELOPE_SCHEMA_VERSION: u32 = 1;
pub(super) const MAX_REPORT_BYTES: u64 = 128 * 1024;
const MAX_PAYLOAD_BYTES: usize = 96 * 1024;
const OPERATION_KEY_BYTES: usize = 32;
const ID_BYTES: usize = 16;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub(super) struct OperationSecurity {
    request_id: String,
    nonce: String,
    key: [u8; OPERATION_KEY_BYTES],
    script_sha256: String,
}

impl OperationSecurity {
    pub(super) fn generate(script_sha256: &str) -> Result<Self, String> {
        if !is_lower_hex(script_sha256, 64) {
            return Err("the prepared script SHA-256 is invalid".to_string());
        }
        let mut request_id = [0_u8; ID_BYTES];
        let mut nonce = [0_u8; ID_BYTES];
        let mut key = [0_u8; OPERATION_KEY_BYTES];
        getrandom::fill(&mut request_id).map_err(|error| error.to_string())?;
        getrandom::fill(&mut nonce).map_err(|error| error.to_string())?;
        getrandom::fill(&mut key).map_err(|error| error.to_string())?;
        Ok(Self {
            request_id: hex_encode(&request_id),
            nonce: hex_encode(&nonce),
            key,
            script_sha256: script_sha256.to_string(),
        })
    }

    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        Self {
            request_id: "00112233445566778899aabbccddeeff".to_string(),
            nonce: "ffeeddccbbaa99887766554433221100".to_string(),
            key: [0x5a; OPERATION_KEY_BYTES],
            script_sha256: "ab".repeat(32),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AuthenticatedPayload {
    pub(super) sequence: u64,
    pub(super) payload: Vec<u8>,
    pub(super) mac: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReportAuthenticationError {
    Oversized,
    InvalidEnvelope(String),
    UnsupportedSchema(u32),
    RequestMismatch,
    NonceMismatch,
    InvalidSequence,
    InvalidPayload(String),
    InvalidMac,
    Replay,
    SequenceReuse,
}

impl fmt::Display for ReportAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized => formatter.write_str("the operation report is too large"),
            Self::InvalidEnvelope(error) => write!(formatter, "invalid report envelope: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported report schema version {version}")
            }
            Self::RequestMismatch => formatter.write_str("the report request ID does not match"),
            Self::NonceMismatch => formatter.write_str("the report nonce does not match"),
            Self::InvalidSequence => formatter.write_str("the report sequence is invalid"),
            Self::InvalidPayload(error) => write!(formatter, "invalid report payload: {error}"),
            Self::InvalidMac => formatter.write_str("the report authentication code is invalid"),
            Self::Replay => formatter.write_str("a stale or replayed report sequence was detected"),
            Self::SequenceReuse => {
                formatter.write_str("a report sequence was reused with different content")
            }
        }
    }
}

#[derive(Default)]
pub(super) struct ReportSequenceTracker {
    last: Option<(u64, [u8; 32])>,
}

impl ReportSequenceTracker {
    pub(super) fn after(report: &AuthenticatedPayload) -> Self {
        Self {
            last: Some((report.sequence, report.mac)),
        }
    }

    pub(super) fn accept(
        &mut self,
        report: &AuthenticatedPayload,
    ) -> Result<bool, ReportAuthenticationError> {
        if let Some((sequence, mac)) = self.last {
            if report.sequence < sequence {
                return Err(ReportAuthenticationError::Replay);
            }
            if report.sequence == sequence {
                return if report.mac == mac {
                    Ok(false)
                } else {
                    Err(ReportAuthenticationError::SequenceReuse)
                };
            }
        }
        self.last = Some((report.sequence, report.mac));
        Ok(true)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReportEnvelope {
    schema_version: u32,
    request_id: String,
    nonce: String,
    sequence: u64,
    payload: String,
    mac: String,
}

pub(super) fn authenticate_report(
    content: &[u8],
    security: &OperationSecurity,
) -> Result<AuthenticatedPayload, ReportAuthenticationError> {
    if content.len() as u64 > MAX_REPORT_BYTES {
        return Err(ReportAuthenticationError::Oversized);
    }
    let content = std::str::from_utf8(content)
        .map_err(|error| ReportAuthenticationError::InvalidEnvelope(error.to_string()))?
        .trim_start_matches('\u{feff}')
        .trim();
    let envelope = serde_json::from_str::<ReportEnvelope>(content)
        .map_err(|error| ReportAuthenticationError::InvalidEnvelope(error.to_string()))?;
    if envelope.schema_version != ENVELOPE_SCHEMA_VERSION {
        return Err(ReportAuthenticationError::UnsupportedSchema(
            envelope.schema_version,
        ));
    }
    if envelope.request_id != security.request_id {
        return Err(ReportAuthenticationError::RequestMismatch);
    }
    if envelope.nonce != security.nonce {
        return Err(ReportAuthenticationError::NonceMismatch);
    }
    if envelope.sequence == 0 {
        return Err(ReportAuthenticationError::InvalidSequence);
    }
    let payload = BASE64_STANDARD
        .decode(&envelope.payload)
        .map_err(|error| ReportAuthenticationError::InvalidPayload(error.to_string()))?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(ReportAuthenticationError::Oversized);
    }
    let mac_bytes = hex_decode(&envelope.mac).ok_or(ReportAuthenticationError::InvalidMac)?;
    let mac: [u8; 32] = mac_bytes
        .try_into()
        .map_err(|_| ReportAuthenticationError::InvalidMac)?;
    let message = authenticated_message(
        &envelope.request_id,
        &envelope.nonce,
        envelope.sequence,
        &envelope.payload,
    );
    let mut verifier =
        HmacSha256::new_from_slice(&security.key).expect("a 256-bit HMAC key is always accepted");
    verifier.update(message.as_bytes());
    verifier
        .verify_slice(&mac)
        .map_err(|_| ReportAuthenticationError::InvalidMac)?;
    Ok(AuthenticatedPayload {
        sequence: envelope.sequence,
        payload,
        mac,
    })
}

pub(super) fn secure_powershell_launcher(
    windows_script_path: &str,
    security: &OperationSecurity,
) -> String {
    let source = powershell_literal(windows_script_path);
    let key = BASE64_STANDARD.encode(security.key);
    format!(
        r#"$ErrorActionPreference='Stop'
$source='{source}'
$expected='{script_sha256}'
$requestId='{request_id}'
$nonce='{nonce}'
$key='{key}'
$applicationRoot=Join-Path $env:ProgramData 'Mendimaru'
$commandRoot=Join-Path $env:ProgramData 'Mendimaru\Commands'
$exitCode=1
try {{
  $programDataItem=Get-Item -LiteralPath $env:ProgramData -Force
  if (($programDataItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw 'MENDIMARU_COMMAND_REPARSE_POINT' }}
  foreach ($directory in @($applicationRoot,$commandRoot)) {{
    if (Test-Path -LiteralPath $directory) {{
      $directoryItem=Get-Item -LiteralPath $directory -Force
      if (-not $directoryItem.PSIsContainer -or ($directoryItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw 'MENDIMARU_COMMAND_REPARSE_POINT' }}
    }} else {{
      New-Item -ItemType Directory -Path $directory | Out-Null
    }}
  }}
  $sourceItem=Get-Item -LiteralPath $source -Force
  if (($sourceItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {{ throw 'MENDIMARU_COMMAND_REPARSE_POINT' }}
  $before=(Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($before -ne $expected) {{ throw 'MENDIMARU_COMMAND_HASH_MISMATCH' }}
  $destination=Join-Path $commandRoot ($requestId + '.ps1')
  if (Test-Path -LiteralPath $destination) {{ throw 'MENDIMARU_COMMAND_DESTINATION_EXISTS' }}
  Copy-Item -LiteralPath $source -Destination $destination
  $after=(Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
  $copied=(Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($after -ne $expected -or $copied -ne $expected) {{ throw 'MENDIMARU_COMMAND_HASH_MISMATCH' }}
  $env:MENDIMARU_REQUEST_ID=$requestId
  $env:MENDIMARU_OPERATION_NONCE=$nonce
  $env:MENDIMARU_OPERATION_KEY=$key
  & $destination
  $exitCode=if ($null -eq $LASTEXITCODE) {{ 0 }} else {{ [int]$LASTEXITCODE }}
}} catch {{
  [Console]::Error.WriteLine($_.Exception.Message)
  $exitCode=1
}} finally {{
  Remove-Item Env:MENDIMARU_REQUEST_ID -ErrorAction SilentlyContinue
  Remove-Item Env:MENDIMARU_OPERATION_NONCE -ErrorAction SilentlyContinue
  Remove-Item Env:MENDIMARU_OPERATION_KEY -ErrorAction SilentlyContinue
  if ($null -ne $destination) {{ Remove-Item -LiteralPath $destination -Force -ErrorAction SilentlyContinue }}
}}
exit $exitCode"#,
        script_sha256 = security.script_sha256,
        request_id = security.request_id,
        nonce = security.nonce,
    )
}

fn authenticated_message(request_id: &str, nonce: &str, sequence: u64, payload: &str) -> String {
    format!("{request_id}\n{nonce}\n{sequence}\n{payload}")
}

pub(super) fn authenticated_envelope(
    security: &OperationSecurity,
    sequence: u64,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    if sequence == 0 {
        return Err("the authenticated payload sequence must be positive".to_string());
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err("the authenticated payload is too large".to_string());
    }
    let payload = BASE64_STANDARD.encode(payload);
    let message = authenticated_message(&security.request_id, &security.nonce, sequence, &payload);
    let mut signer =
        HmacSha256::new_from_slice(&security.key).expect("a 256-bit HMAC key is always accepted");
    signer.update(message.as_bytes());
    let mac = hex_encode(&signer.finalize().into_bytes());
    serde_json::to_vec(&serde_json::json!({
        "schemaVersion": ENVELOPE_SCHEMA_VERSION,
        "requestId": security.request_id,
        "nonce": security.nonce,
        "sequence": sequence,
        "payload": payload,
        "mac": mac,
    }))
    .map_err(|error| format!("could not serialize an authenticated payload: {error}"))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
pub(super) fn authenticated_report_fixture(
    security: &OperationSecurity,
    sequence: u64,
    payload: &[u8],
) -> String {
    String::from_utf8(
        authenticated_envelope(security, sequence, payload)
            .expect("the fixture envelope serializes"),
    )
    .expect("the fixture envelope is UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &[u8] = br#"{"state":"succeeded","message":"ok","percentage":null,"estimated":false,"timestamp":"2026-08-14T00:00:00Z","exitCode":0,"executablePath":null,"error":null}"#;

    #[test]
    fn accepts_an_authenticated_operation_report() {
        let security = OperationSecurity::fixture();
        let envelope = authenticated_report_fixture(&security, 7, PAYLOAD);

        let authenticated = authenticate_report(envelope.as_bytes(), &security)
            .expect("fixture report authenticates");

        assert_eq!(authenticated.sequence, 7);
        assert_eq!(authenticated.payload, PAYLOAD);
    }

    #[test]
    fn rejects_payload_mac_identity_nonce_and_schema_tampering() {
        let security = OperationSecurity::fixture();
        let envelope = authenticated_report_fixture(&security, 1, PAYLOAD);
        let mut value = serde_json::from_str::<serde_json::Value>(&envelope).expect("fixture json");

        for (field, replacement, expected) in [
            (
                "requestId",
                serde_json::json!("11112233445566778899aabbccddeeff"),
                ReportAuthenticationError::RequestMismatch,
            ),
            (
                "nonce",
                serde_json::json!("11112233445566778899aabbccddeeff"),
                ReportAuthenticationError::NonceMismatch,
            ),
            (
                "schemaVersion",
                serde_json::json!(99),
                ReportAuthenticationError::UnsupportedSchema(99),
            ),
        ] {
            let original = value[field].clone();
            value[field] = replacement;
            assert_eq!(
                authenticate_report(value.to_string().as_bytes(), &security),
                Err(expected)
            );
            value[field] = original;
        }

        value["payload"] = serde_json::json!(BASE64_STANDARD.encode(b"changed"));
        assert_eq!(
            authenticate_report(value.to_string().as_bytes(), &security),
            Err(ReportAuthenticationError::InvalidMac)
        );
    }

    #[test]
    fn rejects_zero_sequence_bad_mac_invalid_base64_and_oversized_reports() {
        let security = OperationSecurity::fixture();
        let envelope = authenticated_report_fixture(&security, 1, PAYLOAD);
        let mut value = serde_json::from_str::<serde_json::Value>(&envelope).expect("fixture json");

        value["sequence"] = serde_json::json!(0);
        assert_eq!(
            authenticate_report(value.to_string().as_bytes(), &security),
            Err(ReportAuthenticationError::InvalidSequence)
        );
        value["sequence"] = serde_json::json!(1);
        value["mac"] = serde_json::json!("00".repeat(32));
        assert_eq!(
            authenticate_report(value.to_string().as_bytes(), &security),
            Err(ReportAuthenticationError::InvalidMac)
        );
        value["payload"] = serde_json::json!("%%%invalid%%%");
        assert!(matches!(
            authenticate_report(value.to_string().as_bytes(), &security),
            Err(ReportAuthenticationError::InvalidPayload(_))
        ));
        assert_eq!(
            authenticate_report(&vec![b'X'; MAX_REPORT_BYTES as usize + 1], &security),
            Err(ReportAuthenticationError::Oversized)
        );
    }

    #[test]
    fn secure_launcher_hash_pins_a_private_guest_copy_without_sharing_the_key() {
        let security = OperationSecurity::fixture();
        let launcher = secure_powershell_launcher(
            r"\\host.lan\Data\.mendimaru\commands\operation.ps1",
            &security,
        );

        assert!(launcher.contains("Get-FileHash"));
        assert!(launcher.contains("MENDIMARU_COMMAND_HASH_MISMATCH"));
        assert!(launcher.contains("Mendimaru\\Commands"));
        assert!(launcher.contains("MENDIMARU_OPERATION_KEY"));
        assert!(launcher.contains("ReparsePoint"));
        assert!(!launcher.contains("ExecutionPolicy Bypass"));
        assert!(!launcher.contains("& '\\\\host.lan\\Data"));
    }

    #[test]
    fn rejects_stale_replay_and_sequence_reuse_but_allows_unchanged_polling() {
        let security = OperationSecurity::fixture();
        let first = authenticate_report(
            authenticated_report_fixture(&security, 4, PAYLOAD).as_bytes(),
            &security,
        )
        .expect("first report authenticates");
        let stale = authenticate_report(
            authenticated_report_fixture(&security, 3, PAYLOAD).as_bytes(),
            &security,
        )
        .expect("stale report has a valid MAC");
        let reused = authenticate_report(
            authenticated_report_fixture(&security, 4, b"different").as_bytes(),
            &security,
        )
        .expect("reused sequence has a valid MAC");
        let next = authenticate_report(
            authenticated_report_fixture(&security, 5, PAYLOAD).as_bytes(),
            &security,
        )
        .expect("next report authenticates");

        let mut tracker = ReportSequenceTracker::default();
        assert_eq!(tracker.accept(&first), Ok(true));
        assert_eq!(tracker.accept(&first), Ok(false));
        assert_eq!(
            tracker.accept(&stale),
            Err(ReportAuthenticationError::Replay)
        );
        assert_eq!(
            tracker.accept(&reused),
            Err(ReportAuthenticationError::SequenceReuse)
        );
        assert_eq!(tracker.accept(&next), Ok(true));
    }
}
