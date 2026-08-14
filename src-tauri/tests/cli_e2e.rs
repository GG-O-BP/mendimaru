use mendimaru_lib::models::{AppConfig, ContainerRuntime};
use serde_json::Value;
use std::fs;
use std::process::{Command, Output};

fn fixture_config(workspace: &std::path::Path) -> AppConfig {
    AppConfig {
        language_preference: "en-US".into(),
        winboat_setup_pending: false,
        winboat_executable: "mendimaru-e2e-missing-winboat".into(),
        compose_file: workspace
            .join("missing-compose.yml")
            .to_string_lossy()
            .into_owned(),
        container_runtime: ContainerRuntime::Docker,
        container_name: "MendimaruE2EWinBoat".into(),
        api_url: "http://127.0.0.1:9".into(),
        rdp_host: "127.0.0.1".into(),
        rdp_port: 9,
        shared_directory: workspace.to_string_lossy().into_owned(),
        windows_shared_directory: r"\\host.lan\Data".into(),
        freerdp_binary: "mendimaru-e2e-missing-freerdp".into(),
        mendix_install_root: r"C:\Program Files\Mendix".into(),
        mendix_data_root: r"C:\ProgramData\Mendix".into(),
        windows_studio_paths: Vec::new(),
        startup_timeout_seconds: 1,
    }
}

fn run(config_directory: &std::path::Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mendimaru"))
        .args(arguments)
        .env("MENDIMARU_CONFIG_DIR", config_directory)
        .env(
            "MENDIMARU_CACHE_DIR",
            config_directory.join("isolated-cache"),
        )
        .output()
        .expect("run the real mendimaru binary")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "success wrote diagnostics");
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

fn stderr_json(output: &Output) -> Value {
    assert!(!output.status.success(), "command unexpectedly succeeded");
    assert!(output.stdout.is_empty(), "failure polluted stdout");
    serde_json::from_slice(&output.stderr).expect("stderr is one JSON document")
}

#[test]
fn real_binary_lists_and_versions_projects_without_disclosing_paths() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let config_directory = temporary.path().join("config");
    let workspace = temporary.path().join("workspace");
    let project_directory = workspace.join("Orders");
    fs::create_dir_all(&config_directory).expect("config directory");
    fs::create_dir_all(&project_directory).expect("project directory");
    fs::write(project_directory.join("Orders.mpr"), b"mpr fixture").expect("mpr fixture");
    fs::write(
        project_directory.join("project-settings.user.json"),
        r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.12.2.0"}]}"#,
    )
    .expect("settings fixture");
    fs::write(
        config_directory.join("config.json"),
        serde_json::to_vec_pretty(&fixture_config(&workspace)).expect("serialize config"),
    )
    .expect("config fixture");

    let listed = stdout_json(&run(&config_directory, &["project", "list", "--json"]));
    assert_complete_envelope(&listed, "project.list");
    let serialized = serde_json::to_string(&listed).expect("serialize response");
    assert!(!serialized.contains(temporary.path().to_string_lossy().as_ref()));
    assert!(!serialized.contains("mprPath"));
    assert!(!serialized.contains("windowsPath"));
    let project_id = listed["data"][0]["projectId"]
        .as_str()
        .expect("opaque project ID");
    assert!(project_id.starts_with("project_"));

    let versioned = stdout_json(&run(
        &config_directory,
        &["project", "version", "--project-id", project_id],
    ));
    assert_complete_envelope(&versioned, "project.version");
    assert_eq!(versioned["data"]["projectId"], project_id);
    assert_eq!(versioned["data"]["requiredVersion"], "11.12.2");
}

#[test]
fn real_binary_rejects_a_project_version_mismatch_before_mutation() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let config_directory = temporary.path().join("config");
    let workspace = temporary.path().join("workspace");
    let project_directory = workspace.join("Orders");
    fs::create_dir_all(&config_directory).expect("config directory");
    fs::create_dir_all(&project_directory).expect("project directory");
    let mpr = project_directory.join("Orders.mpr");
    fs::write(&mpr, b"mpr fixture must remain unchanged").expect("mpr fixture");
    fs::write(
        project_directory.join("project-settings.user.json"),
        r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.12.2.0"}]}"#,
    )
    .expect("settings fixture");
    fs::write(
        config_directory.join("config.json"),
        serde_json::to_vec_pretty(&fixture_config(&workspace)).expect("serialize config"),
    )
    .expect("config fixture");
    let before = fs::read(&mpr).expect("read fixture before command");
    let listed = stdout_json(&run(&config_directory, &["project", "list"]));
    let project_id = listed["data"][0]["projectId"].as_str().expect("project ID");

    let rejected = run(
        &config_directory,
        &[
            "studio",
            "start",
            "--version",
            "11.13.0",
            "--project-id",
            project_id,
            "--json",
        ],
    );
    assert_eq!(rejected.status.code(), Some(1));
    let error = stderr_json(&rejected);
    assert_complete_envelope(&error, "studio.start");
    assert_eq!(error["error"]["code"], "precondition_failed");
    assert_eq!(fs::read(&mpr).expect("read fixture after command"), before);
    assert!(!config_directory.join("operation-history.json").exists());
    assert!(!workspace.join(".mendimaru").exists());
}

#[test]
fn real_binary_keeps_results_and_diagnostics_separate_for_every_outcome() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let config_directory = temporary.path().join("config");
    let workspace = temporary.path().join("workspace");
    fs::create_dir_all(&config_directory).expect("config directory");
    fs::create_dir_all(&workspace).expect("workspace directory");
    fs::write(
        config_directory.join("config.json"),
        serde_json::to_vec_pretty(&fixture_config(&workspace)).expect("serialize config"),
    )
    .expect("config fixture");

    for (arguments, command) in [
        (vec!["capabilities", "--json"], "capabilities"),
        (vec!["env", "status", "--json"], "env.status"),
        (vec!["project", "list", "--json"], "project.list"),
        (vec!["operation", "list", "--json"], "operation.list"),
    ] {
        let document = stdout_json(&run(&config_directory, &arguments));
        assert_complete_envelope(&document, command);
    }

    let secret = "fixture-password-that-must-not-leak";
    let invalid = run(&config_directory, &["studio", "list", "--password", secret]);
    assert_eq!(invalid.status.code(), Some(2));
    let error = stderr_json(&invalid);
    assert_complete_envelope(&error, "studio");
    assert_eq!(error["error"]["code"], "invalid_request");
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains(secret));
}

fn assert_complete_envelope(document: &Value, command: &str) {
    assert_eq!(document["schemaVersion"], "1.0.0");
    assert_eq!(document["command"], command);
    assert!(document["ok"].is_boolean());
    assert!(document["platform"].is_string());
    assert!(document["backend"].is_string());
    assert!(document["sessionId"]
        .as_str()
        .is_some_and(|value| value.starts_with("session_")));
    assert!(document["capabilitySnapshot"].is_object());
}
