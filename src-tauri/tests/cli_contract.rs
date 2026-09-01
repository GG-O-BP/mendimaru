use serde_json::Value;
use std::collections::BTreeSet;
use std::process::{Command, Output};

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mendimaru"))
        .args(arguments)
        .output()
        .expect("run the built Mendimaru binary")
}

fn current_backend() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux-winboat"
    } else if cfg!(target_os = "windows") {
        "windows-native"
    } else if cfg!(target_os = "macos") {
        "mac-native"
    } else {
        "unsupported"
    }
}

fn mismatched_backend() -> &'static str {
    if cfg!(target_os = "linux") {
        "windows-native"
    } else if cfg!(target_os = "windows") {
        "mac-native"
    } else {
        "linux-winboat"
    }
}

#[test]
fn real_binary_prints_root_help_without_a_capability_snapshot() {
    for help in ["--help", "-h"] {
        let output = run(&[help]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
        assert!(stdout.contains("Mendimaru headless CLI"));
        assert!(stdout.contains("env status"));
        assert!(stdout.contains("Exit codes: 0 success"));
        assert!(!stdout.contains("capabilitySnapshot"));
        assert!(!stdout.contains("snapshotId"));
    }
}

#[test]
fn real_binary_rejects_unknown_commands_immediately_without_backend_context() {
    let output = run(&["status", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    let json: Value = serde_json::from_str(&stderr).expect("stderr is one JSON document");
    assert_eq!(json["command"], "unknown");
    assert_eq!(json["ok"], false);
    assert_eq!(json["sessionId"], "session_unavailable");
    assert_eq!(json["capabilitySnapshot"], Value::Null);
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(json["error"]["retryable"], false);
    assert!(json["error"]["message"]
        .as_str()
        .expect("message is a string")
        .contains("'env status'"));
}

#[test]
fn real_binary_prints_subcommand_help_without_machine_readable_errors() {
    for command in [
        "runtime start",
        "runtime wait",
        "studio status",
        "browser install",
        "operation retry",
    ] {
        let mut arguments = command.split(' ').collect::<Vec<_>>();
        arguments.push("--help");
        let output = run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
        assert!(stdout.contains(&format!("Usage: mendimaru {command}")));
        assert!(!stdout.contains("schemaVersion"));
        assert!(!stdout.contains("snapshotId"));
    }
}

#[test]
fn real_binary_emits_a_complete_platform_neutral_capability_snapshot() {
    let output = run(&["capabilities", "--json", "--backend", current_backend()]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert_eq!(stdout.lines().count(), 1);
    let json: Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
    assert_eq!(json["schemaVersion"], "4.0.0");
    assert_eq!(json["command"], "capabilities");
    assert_eq!(json["ok"], true);
    assert_eq!(json["data"]["manifest"]["backend"], current_backend());

    let manifest = json["data"]["manifest"]
        .as_object()
        .expect("manifest is an object");
    for winboat_specific in [
        "apiUrl",
        "rdpHost",
        "rdpPort",
        "composeFile",
        "windowsSharedDirectory",
    ] {
        assert!(
            !manifest.contains_key(winboat_specific),
            "common manifest leaked {winboat_specific}"
        );
    }

    let capabilities = manifest["capabilities"]
        .as_array()
        .expect("capabilities is an array");
    assert_eq!(capabilities.len(), 21);
    let ids = capabilities
        .iter()
        .map(|capability| {
            capability["id"]
                .as_str()
                .expect("capability ID is a string")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), capabilities.len());
    assert!(ids.contains("studio.detect"));
    assert!(ids.contains("runtime.url"));
    assert!(ids.contains("ui.screenshot"));
    assert!(ids.contains("browser.artifacts"));
    assert!(capabilities
        .iter()
        .all(|capability| capability["fallbackAllowed"] == false));
}

#[test]
fn real_binary_uses_fresh_snapshot_ids_for_separate_calls() {
    let first = run(&["capabilities", "--json"]);
    let second = run(&["capabilities", "--json"]);
    assert!(first.status.success());
    assert!(second.status.success());
    let first: Value = serde_json::from_slice(&first.stdout).expect("first response is JSON");
    let second: Value = serde_json::from_slice(&second.stdout).expect("second response is JSON");
    assert_ne!(first["data"]["snapshotId"], second["data"]["snapshotId"]);
}

#[test]
fn real_binary_rejects_cross_platform_override_without_stdout_or_fallback() {
    let output = run(&["capabilities", "--json", "--backend", mismatched_backend()]);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert_eq!(stderr.lines().count(), 1);
    let json: Value = serde_json::from_str(&stderr).expect("stderr is one JSON document");
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["code"], "backend_mismatch");
    assert_eq!(json["error"]["backend"], mismatched_backend());
}

#[test]
fn real_binary_rejects_unknown_flags_as_machine_readable_invalid_request() {
    let output = run(&["capabilities", "--json", "--guess-platform"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let json: Value =
        serde_json::from_slice(&output.stderr).expect("stderr is a JSON error envelope");
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(json["error"]["retryable"], false);
}
