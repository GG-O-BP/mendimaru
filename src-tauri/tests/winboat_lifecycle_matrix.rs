#![cfg(target_os = "linux")]

mod support;

use serde_json::Value;
use std::fs;
use support::winboat_lifecycle::{
    ComposeVariant, WinBoatLifecycleFixture, LEGACY_SCHEMA, LEGACY_SESSION_ID,
};

#[test]
fn winboat_upgrade_and_orphan_lifecycle_matrix_stays_secret_free() {
    let fixture = WinBoatLifecycleFixture::new();
    let active_id = fixture.write_current_runtime_record('a', "starting", None);
    let stopped_id = fixture.write_current_runtime_record('b', "stopped", None);
    let linked_id =
        fixture.write_current_runtime_record('c', "ready", Some("studio-4242-639240248846965363"));
    let legacy_id = fixture.write_legacy_runtime_record();

    for variant in [
        ComposeVariant::Clean,
        ComposeVariant::Dynamic,
        ComposeVariant::Public,
        ComposeVariant::FixedStale,
    ] {
        fixture.write_compose(variant);
        let compose = fixture.compose_text();
        assert!(compose.contains("127.0.0.1:47280:7148/tcp"));
        assert!(compose.contains("127.0.0.1:5900:5900/tcp"));
        assert!(compose.contains("winboat-data:/storage"));
    }

    let lock = fixture.write_dead_lock(4242);
    assert!(lock.is_file());
    let orphan_socket = fixture.write_orphan_keeper_socket(&format!("s-{}.sock", "d".repeat(32)));
    assert!(orphan_socket.exists());

    let listed = stdout_json(&fixture.run_cli(&["runtime", "list", "--json"]));
    let sessions = listed["data"]["sessions"]
        .as_array()
        .expect("Runtime session summaries");
    assert_eq!(sessions.len(), 4);
    let legacy = find_session(sessions, &legacy_id);
    assert_eq!(legacy["incompatibleRecord"], true);
    assert_eq!(legacy["incompatibilityReason"], "schema_version_mismatch");
    assert_eq!(legacy["forgetEligible"], true);
    let active = find_session(sessions, &active_id);
    assert_eq!(active["state"], "starting");
    assert_eq!(active["forgetEligible"], false);
    let stopped = find_session(sessions, &stopped_id);
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["forgetEligible"], true);
    let linked = find_session(sessions, &linked_id);
    assert_eq!(linked["studioSessionId"], "studio-4242-639240248846965363");
    let serialized = serde_json::to_string(&listed).expect("serialized list");
    assert!(!serialized.contains(fixture.workspace.to_string_lossy().as_ref()));
    assert!(!serialized.contains(fixture.cache_directory.to_string_lossy().as_ref()));
    assert!(!serialized.contains("compose.original.yml"));

    let legacy_status =
        fixture.run_cli(&["runtime", "status", "--session-id", &legacy_id, "--json"]);
    assert_eq!(legacy_status.status.code(), Some(1));
    let error = stderr_json(&legacy_status);
    assert_eq!(error["error"]["code"], "runtime_session_not_found");
    assert_eq!(error["error"]["retryable"], false);
    assert!(!String::from_utf8_lossy(&legacy_status.stderr)
        .contains(fixture.cache_directory.to_string_lossy().as_ref()));

    let rejected = fixture.run_cli(&["runtime", "forget", "--session-id", &active_id, "--json"]);
    assert_eq!(rejected.status.code(), Some(1));
    let rejected_error = stderr_json(&rejected);
    assert_eq!(rejected_error["error"]["code"], "precondition_failed");
    assert!(fixture.runtime_record_path(&active_id).is_file());
    assert!(
        fixture.runtime_record_path(&legacy_id).is_file(),
        "legacy record disappeared before forget"
    );

    let forgotten =
        stdout_json(&fixture.run_cli(&["runtime", "forget", "--session-id", &legacy_id, "--json"]));
    assert_eq!(forgotten["data"]["sessionId"], legacy_id);
    assert_eq!(forgotten["data"]["forgotten"], true);
    let legacy_directory = fixture.runtime_session_directory(&legacy_id);
    assert!(!legacy_directory.join("session.json").exists());
    assert!(legacy_directory.join("session.invalidated.json").is_file());
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(legacy_directory.join("invalidation.json"))
            .expect("legacy invalidation marker"),
    )
    .expect("legacy invalidation JSON");
    assert_eq!(marker["schemaVersion"], "4.0.0");
    assert_eq!(marker["reason"], "schema_version_mismatch");
    assert_eq!(legacy["sessionId"], LEGACY_SESSION_ID);

    let repeat =
        stdout_json(&fixture.run_cli(&["runtime", "forget", "--session-id", &legacy_id, "--json"]));
    assert_eq!(repeat["data"]["forgotten"], true);
    let preserved: Value = serde_json::from_str(
        &fs::read_to_string(legacy_directory.join("session.invalidated.json"))
            .expect("preserved legacy record"),
    )
    .expect("preserved legacy JSON");
    assert_eq!(preserved["schemaVersion"], LEGACY_SCHEMA);
    let listed_again = stdout_json(&fixture.run_cli(&["runtime", "list", "--json"]));
    let sessions_again = listed_again["data"]["sessions"]
        .as_array()
        .expect("Runtime summaries after invalidation");
    assert_eq!(
        find_session(sessions_again, &legacy_id)["incompatibleRecord"],
        true
    );
}

fn stdout_json(output: &std::process::Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("successful CLI JSON")
}

fn stderr_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stderr).expect("structured error")
}

fn find_session<'a>(sessions: &'a [Value], session_id: &str) -> &'a Value {
    sessions
        .iter()
        .find(|session| session["sessionId"] == session_id)
        .unwrap_or_else(|| panic!("missing Runtime summary for {session_id}"))
}
