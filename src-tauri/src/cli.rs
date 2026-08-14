use crate::contracts::{
    BackendError, BackendErrorCode, BackendId, CapabilitySnapshot, CONTRACT_SCHEMA_VERSION,
};
use serde::Serialize;
use std::ffi::OsString;
use std::io::Write;
use std::str::FromStr;

const EXIT_OK: i32 = 0;
const EXIT_OPERATION_FAILED: i32 = 1;
const EXIT_INVALID_REQUEST: i32 = 2;
const EXIT_BACKEND_UNAVAILABLE: i32 = 3;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SuccessEnvelope<T> {
    schema_version: &'static str,
    command: &'static str,
    ok: bool,
    data: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope {
    schema_version: &'static str,
    command: &'static str,
    ok: bool,
    error: BackendError,
}

#[derive(Debug, PartialEq, Eq)]
struct CliExecution {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// Runs a recognized headless command before Tauri is initialized. Returning
/// `None` means that the process should continue as the desktop application.
pub fn dispatch_from_env() -> Option<i32> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let execution = execute(&arguments)?;
    if !execution.stdout.is_empty() {
        let _ = std::io::stdout().write_all(execution.stdout.as_bytes());
    }
    if !execution.stderr.is_empty() {
        let _ = std::io::stderr().write_all(execution.stderr.as_bytes());
    }
    Some(execution.exit_code)
}

fn execute(arguments: &[OsString]) -> Option<CliExecution> {
    if arguments.first().and_then(|value| value.to_str()) != Some("capabilities") {
        return None;
    }

    let requested = match parse_capability_arguments(&arguments[1..]) {
        Ok(requested) => requested,
        Err(error) => return Some(error_execution(error)),
    };
    Some(match crate::platform::capability_snapshot(requested) {
        Ok(snapshot) => success_execution(snapshot),
        Err(error) => error_execution(error),
    })
}

fn parse_capability_arguments(arguments: &[OsString]) -> Result<Option<BackendId>, BackendError> {
    let mut requested = None;
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments[index]
            .to_str()
            .ok_or_else(|| BackendError::invalid_request("CLI arguments must be valid UTF-8"))?;
        match value {
            "--json" => {}
            "--backend" => {
                if requested.is_some() {
                    return Err(BackendError::invalid_request(
                        "--backend may only be provided once",
                    ));
                }
                index += 1;
                let backend = arguments
                    .get(index)
                    .and_then(|argument| argument.to_str())
                    .ok_or_else(|| {
                        BackendError::invalid_request("--backend requires a backend ID")
                    })?;
                requested = Some(BackendId::from_str(backend).map_err(|_| {
                    BackendError::invalid_request(format!(
                        "unknown backend {backend}; expected linux-winboat, windows-native, or mac-native"
                    ))
                })?);
            }
            _ if value.starts_with("--backend=") => {
                if requested.is_some() {
                    return Err(BackendError::invalid_request(
                        "--backend may only be provided once",
                    ));
                }
                let backend = value.trim_start_matches("--backend=");
                requested = Some(BackendId::from_str(backend).map_err(|_| {
                    BackendError::invalid_request(format!(
                        "unknown backend {backend}; expected linux-winboat, windows-native, or mac-native"
                    ))
                })?);
            }
            _ => {
                return Err(BackendError::invalid_request(format!(
                    "unknown capabilities argument: {value}"
                )));
            }
        }
        index += 1;
    }
    Ok(requested)
}

fn success_execution(snapshot: CapabilitySnapshot) -> CliExecution {
    let envelope = SuccessEnvelope {
        schema_version: CONTRACT_SCHEMA_VERSION,
        command: "capabilities",
        ok: true,
        data: snapshot,
    };
    CliExecution {
        exit_code: EXIT_OK,
        stdout: json_line(&envelope),
        stderr: String::new(),
    }
}

fn error_execution(error: BackendError) -> CliExecution {
    let exit_code = match error.code {
        BackendErrorCode::InvalidRequest => EXIT_INVALID_REQUEST,
        BackendErrorCode::BackendMismatch | BackendErrorCode::UnsupportedCapability => {
            EXIT_BACKEND_UNAVAILABLE
        }
        BackendErrorCode::PreconditionFailed | BackendErrorCode::OperationFailed => {
            EXIT_OPERATION_FAILED
        }
    };
    let envelope = ErrorEnvelope {
        schema_version: CONTRACT_SCHEMA_VERSION,
        command: "capabilities",
        ok: false,
        error,
    };
    CliExecution {
        exit_code,
        stdout: String::new(),
        stderr: json_line(&envelope),
    }
}

fn json_line<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|error| {
        format!(
            "{{\"schemaVersion\":\"{CONTRACT_SCHEMA_VERSION}\",\"command\":\"capabilities\",\"ok\":false,\"error\":{{\"code\":\"operation_failed\",\"message\":\"JSON serialization failed: {error}\"}}}}"
        )
    });
    format!("{json}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{CapabilityId, PlatformId};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn unrelated_arguments_continue_to_the_desktop_app() {
        assert_eq!(execute(&[]), None);
        assert_eq!(execute(&args(&["project.mpr"])), None);
    }

    #[test]
    fn capabilities_returns_one_json_line_without_tauri_initialization() {
        let execution =
            execute(&args(&["capabilities", "--json"])).expect("capabilities is a CLI command");
        assert_eq!(execution.exit_code, EXIT_OK);
        assert!(execution.stderr.is_empty());
        assert_eq!(execution.stdout.lines().count(), 1);
        let json: serde_json::Value =
            serde_json::from_str(&execution.stdout).expect("stdout is JSON");
        assert_eq!(json["schemaVersion"], CONTRACT_SCHEMA_VERSION);
        assert_eq!(json["ok"], true);
        assert_eq!(
            json["data"]["manifest"]["hostPlatform"],
            serde_json::to_value(PlatformId::current()).expect("platform serializes")
        );
        assert_eq!(
            json["data"]["manifest"]["capabilities"]
                .as_array()
                .expect("capability list")
                .len(),
            CapabilityId::ALL.len()
        );
        assert!(json["data"]["snapshotId"]
            .as_str()
            .is_some_and(|id| id.starts_with("cap_")));
    }

    #[test]
    fn a_mismatched_explicit_backend_is_json_error_on_stderr() {
        let mismatched = match PlatformId::current() {
            PlatformId::Linux => "windows-native",
            PlatformId::Windows => "mac-native",
            PlatformId::Macos => "linux-winboat",
            PlatformId::Unsupported => "linux-winboat",
        };
        let execution = execute(&args(&["capabilities", "--json", "--backend", mismatched]))
            .expect("capabilities is a CLI command");
        assert_eq!(execution.exit_code, EXIT_BACKEND_UNAVAILABLE);
        assert!(execution.stdout.is_empty());
        let json: serde_json::Value =
            serde_json::from_str(&execution.stderr).expect("stderr is JSON");
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "backend_mismatch");
        assert_eq!(json["error"]["backend"], mismatched);
    }

    #[test]
    fn invalid_or_duplicate_backend_arguments_fail_without_guessing() {
        for arguments in [
            args(&["capabilities", "--backend", "future-backend"]),
            args(&[
                "capabilities",
                "--backend=linux-winboat",
                "--backend=windows-native",
            ]),
            args(&["capabilities", "--unknown"]),
        ] {
            let execution = execute(&arguments).expect("capabilities is a CLI command");
            assert_eq!(execution.exit_code, EXIT_INVALID_REQUEST);
            assert!(execution.stdout.is_empty());
            let json: serde_json::Value =
                serde_json::from_str(&execution.stderr).expect("stderr is JSON");
            assert_eq!(json["error"]["code"], "invalid_request");
        }
    }
}
