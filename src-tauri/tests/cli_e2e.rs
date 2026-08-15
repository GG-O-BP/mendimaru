use mendimaru_lib::models::{AppConfig, ContainerRuntime};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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
    run_with_environment(config_directory, arguments, &[])
}

fn run_with_environment(
    config_directory: &Path,
    arguments: &[&str],
    environment: &[(&str, &std::ffi::OsStr)],
) -> Output {
    let command_name = arguments
        .iter()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("E2E CLI started: {command_name}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_mendimaru"));
    command
        .args(arguments)
        .env("MENDIMARU_CONFIG_DIR", config_directory)
        .env(
            "MENDIMARU_CACHE_DIR",
            config_directory.join("isolated-cache"),
        );
    for (name, value) in environment {
        command.env(name, value);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("run the real mendimaru binary");
    let mut stdout = child.stdout.take().expect("capture CLI stdout");
    let mut stderr = child.stderr.take().expect("capture CLI stderr");
    let (stdout_sender, stdout_receiver) = mpsc::channel();
    let (stderr_sender, stderr_receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes).map(|_| bytes);
        let _ = stdout_sender.send(result);
    });
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr.read_to_end(&mut bytes).map(|_| bytes);
        let _ = stderr_sender.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(40);
    let status = loop {
        if let Some(status) = child.try_wait().expect("observe CLI process") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("CLI command {command_name:?} exceeded the 40-second E2E boundary");
        }
        thread::sleep(Duration::from_millis(100));
    };
    let output = Output {
        status,
        stdout: stdout_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("CLI stdout reached EOF")
            .expect("read CLI stdout"),
        stderr: stderr_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("CLI stderr reached EOF")
            .expect("read CLI stderr"),
    };
    eprintln!("E2E CLI completed: {command_name}");
    output
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

    let invalid_runtime = run(
        &config_directory,
        &["runtime", "start", "--password", secret, "--json"],
    );
    assert_eq!(invalid_runtime.status.code(), Some(2));
    let error = stderr_json(&invalid_runtime);
    assert_complete_envelope(&error, "runtime");
    assert_eq!(error["error"]["code"], "invalid_request");
    assert!(!String::from_utf8_lossy(&invalid_runtime.stderr).contains(secret));
}

struct PortableFixture {
    config_directory: PathBuf,
    toolchain: PathBuf,
    java_home: PathBuf,
}

fn portable_fixture(root: &Path, mxbuild_mode: &str, runtime_source: &str) -> PortableFixture {
    let config_directory = root.join("config");
    let workspace = root.join("workspace");
    let project_directory = workspace.join("Orders");
    let toolchain = root.join("toolchains").join("11.12.2");
    let java_home = root.join("java-21");
    fs::create_dir_all(&config_directory).expect("config directory");
    fs::create_dir_all(&project_directory).expect("project directory");
    fs::create_dir_all(&toolchain).expect("toolchain directory");
    fs::create_dir_all(java_home.join("bin")).expect("Java directory");
    fs::write(
        project_directory.join("Orders.mpr"),
        b"portable mpr fixture",
    )
    .expect("mpr fixture");
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

    let helper = compile_fixture_helper(root);
    let executable_suffix = if cfg!(windows) { ".exe" } else { "" };
    for name in ["mx", "mxbuild"] {
        let destination = toolchain.join(format!("{name}{executable_suffix}"));
        fs::copy(&helper, &destination).expect("copy fake toolchain executable");
        make_executable(&destination);
    }
    let java = java_home
        .join("bin")
        .join(format!("java{executable_suffix}"));
    fs::copy(&helper, &java).expect("copy fake Java executable");
    make_executable(&java);
    fs::write(toolchain.join("mxbuild-mode"), mxbuild_mode).expect("MxBuild mode");
    write_portable_package(&toolchain.join("package.zip"), runtime_source);

    PortableFixture {
        config_directory,
        toolchain,
        java_home,
    }
}

fn compile_fixture_helper(root: &Path) -> PathBuf {
    let source = root.join("fixture-tool.rs");
    fs::write(
        &source,
        r##"
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn argument_path(prefix: &str) -> PathBuf {
    std::env::args_os()
        .skip(1)
        .find_map(|argument| {
            let value = argument.to_string_lossy();
            value.strip_prefix(prefix).map(PathBuf::from)
        })
        .expect("fixture argument")
}

fn main() {
    let executable = std::env::current_exe().expect("current fixture executable");
    if std::env::args().nth(1).as_deref() == Some("fixture-child") {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let name = executable
        .file_stem()
        .expect("fixture executable name")
        .to_string_lossy()
        .to_ascii_lowercase();
    let directory = executable.parent().expect("fixture executable directory");
    match name.as_str() {
        "mx" => match std::env::args().nth(1).as_deref() {
            Some("show-version") => println!("11.12.2"),
            Some("show-java-version") => println!("Java 21"),
            _ => std::process::exit(2),
        },
        "java" => eprintln!("openjdk version \"21.0.1\""),
        "mxbuild" => {
            std::thread::sleep(Duration::from_millis(250));
            let mode = fs::read_to_string(directory.join("mxbuild-mode"))
                .unwrap_or_else(|_| "success".to_string());
            if mode.trim() == "hang" {
                let child = Command::new(&executable)
                    .arg("fixture-child")
                    .spawn()
                    .expect("fixture descendant");
                fs::write(directory.join("mxbuild.pid"), std::process::id().to_string())
                    .expect("fixture parent PID");
                fs::write(directory.join("mxbuild-child.pid"), child.id().to_string())
                    .expect("fixture child PID");
                std::thread::sleep(Duration::from_secs(30));
                return;
            }
            let errors = argument_path("--write-errors=");
            let output = argument_path("--output=");
            if mode.trim() == "consistency" {
                fs::write(
                    errors,
                    r#"{"problems":[{"severity":"error","message":"fixture consistency failure"}]}"#,
                )
                .expect("consistency report");
                std::process::exit(1);
            }
            fs::write(errors, r#"{"problems":[]}"#).expect("consistency report");
            if mode.trim() == "build" {
                std::process::exit(1);
            }
            fs::copy(directory.join("package.zip"), output).expect("portable package");
        }
        _ => std::process::exit(2),
    }
}
"##,
    )
    .expect("fixture helper source");
    let executable = root.join(if cfg!(windows) {
        "fixture-tool.exe"
    } else {
        "fixture-tool"
    });
    let status = Command::new(
        std::env::var_os("RUSTC").unwrap_or_else(|| std::ffi::OsString::from("rustc")),
    )
    .arg(&source)
    .arg("-o")
    .arg(&executable)
    .status()
    .expect("compile fixture helper");
    assert!(status.success(), "fixture helper compilation failed");
    make_executable(&executable);
    executable
}

fn write_portable_package(path: &Path, runtime_source: &str) {
    let file = fs::File::create(path).expect("portable package");
    let mut archive = zip::ZipWriter::new(file);
    let file_options = zip::write::SimpleFileOptions::default().unix_permissions(0o600);
    let executable_options = zip::write::SimpleFileOptions::default().unix_permissions(0o700);
    for directory in [
        "app/",
        "app/data/",
        "app/data/database/",
        "bin/",
        "etc/",
        "lib/",
    ] {
        archive
            .add_directory(directory, file_options)
            .expect("portable directory");
    }
    archive
        .start_file("bin/start", executable_options)
        .expect("Linux launcher");
    archive
        .write_all(b"#!/bin/sh\nexec node \"$(dirname \"$0\")/fake-runtime.cjs\" \"$@\"\n")
        .expect("Linux launcher contents");
    archive
        .start_file("bin/start.bat", executable_options)
        .expect("Windows batch launcher");
    archive
        .write_all(b"@echo off\r\nnode.exe \"%~dp0fake-runtime.cjs\" %*\r\n")
        .expect("Windows batch contents");
    archive
        .start_file("bin/start.ps1", executable_options)
        .expect("Windows PowerShell launcher");
    archive
        .write_all(
            b"$ErrorActionPreference = 'Stop'\r\n[Console]::Out.WriteLine('fixture PowerShell launcher entered')\r\ntry {\r\n  $runtime = Join-Path $PSScriptRoot 'fake-runtime.cjs'\r\n  & node.exe $runtime @args\r\n  exit $LASTEXITCODE\r\n} catch {\r\n  [Console]::Error.WriteLine('fixture PowerShell launcher failed: ' + $_.Exception.GetType().FullName)\r\n  exit 127\r\n}\r\n",
        )
        .expect("Windows PowerShell contents");
    archive
        .start_file("bin/fake-runtime.cjs", file_options)
        .expect("fake runtime");
    archive
        .write_all(runtime_source.as_bytes())
        .expect("fake runtime contents");
    archive
        .start_file("etc/Default", file_options)
        .expect("default config");
    archive
        .write_all(b"# fixture default\n")
        .expect("default config contents");
    archive.finish().expect("finish portable package");
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("executable permission");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn run_portable(
    fixture: &PortableFixture,
    arguments: &[&str],
    runtime_environment: Option<&str>,
) -> Output {
    let mut environment = vec![
        ("MENDIMARU_MXBUILD_HOME", fixture.toolchain.as_os_str()),
        ("MENDIMARU_JAVA_HOME", fixture.java_home.as_os_str()),
    ];
    if let Some(value) = runtime_environment {
        environment.push(("MENDIMARU_RUNTIME_ENV_JSON", std::ffi::OsStr::new(value)));
    }
    run_with_environment(&fixture.config_directory, arguments, &environment)
}

fn fixture_runtime_logs(fixture: &PortableFixture) -> String {
    let sessions = fixture
        .config_directory
        .join("isolated-cache")
        .join("portable-runtime")
        .join("sessions");
    let Ok(entries) = fs::read_dir(sessions) else {
        return "no Runtime session directory".to_string();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path().join("runtime.log")).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixture_project_id(fixture: &PortableFixture) -> String {
    let listed = stdout_json(&run_portable(fixture, &["project", "list", "--json"], None));
    listed["data"][0]["projectId"]
        .as_str()
        .expect("portable fixture project ID")
        .to_string()
}

fn assert_http_ok(url: &str) {
    let authority = url
        .strip_prefix("http://")
        .and_then(|value| value.split('/').next())
        .expect("HTTP fixture URL");
    let mut stream = std::net::TcpStream::connect(authority).expect("connect to fake runtime");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("HTTP request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("HTTP response");
    assert!(
        response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),
        "unexpected HTTP response: {response}"
    );
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .is_some_and(|pid| unsafe { libc::kill(pid, 0) == 0 })
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
        .ok()
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
        })
}

fn assert_process_stops(pid: u32) {
    for _ in 0..50 {
        if !process_is_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("fixture process {pid} survived its containment boundary");
}

const READY_RUNTIME: &str = r#"
const fs = require("fs");
const http = require("http");
const config = JSON.parse(fs.readFileSync(process.argv.at(-1), "utf8"));
const secret = process.env.E2E_SECRET;
if (secret) {
  console.log("raw=" + secret);
  console.log("encoded=" + Buffer.from(secret).toString("base64"));
}
const runtime = http.createServer((_request, response) => {
  response.writeHead(200, { "content-type": "text/plain" });
  response.end("ready");
});
const admin = http.createServer((request, response) => {
  response.writeHead(request.url === "/probes/ready" ? 200 : 404);
  response.end();
});
runtime.listen(config.runtime.http.port, "127.0.0.1");
admin.listen(config.admin.port, "127.0.0.1");
const stop = () => {
  runtime.close();
  admin.close(() => process.exit(0));
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
setInterval(() => {}, 1000);
"#;

const NEVER_READY_RUNTIME: &str = r#"
console.log("fixture runtime intentionally never becomes ready");
setInterval(() => {}, 1000);
"#;

#[test]
fn real_binary_builds_starts_observes_redacts_and_stops_portable_runtime() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    // Preserve this fixture while unwinding so a still-running Windows Runtime
    // cannot hide the original assertion behind TempDir cleanup.
    let fixture_root = temporary.keep();
    let fixture = portable_fixture(&fixture_root, "success", READY_RUNTIME);
    let project_id = fixture_project_id(&fixture);

    let first_build = stdout_json(&run_portable(
        &fixture,
        &["runtime", "build", "--project-id", &project_id, "--json"],
        None,
    ));
    assert_complete_envelope(&first_build, "runtime.build");
    assert_eq!(first_build["data"]["cacheHit"], false);
    assert_eq!(first_build["data"]["requiredVersion"], "11.12.2");
    assert_eq!(first_build["data"]["toolchainVersion"], "11.12.2");
    assert_eq!(
        first_build["data"]["packageArtifact"]["kind"],
        "runtime-package"
    );
    assert_eq!(
        first_build["data"]["consistencyArtifact"]["kind"],
        "consistency-report"
    );
    assert_eq!(first_build["data"]["buildLogArtifact"]["kind"], "build-log");

    let cached_build = stdout_json(&run_portable(
        &fixture,
        &["runtime", "build", "--project-id", &project_id, "--json"],
        None,
    ));
    assert_eq!(cached_build["data"]["cacheHit"], true);
    assert_eq!(
        cached_build["data"]["packageArtifact"]["sha256"],
        first_build["data"]["packageArtifact"]["sha256"]
    );

    let secret = "portable-secret-7f6c91";
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        secret.as_bytes(),
    );
    let environment = format!(r#"{{"E2E_SECRET":"{secret}"}}"#);
    let started_output = run_portable(
        &fixture,
        &[
            "runtime",
            "start",
            "--project-id",
            &project_id,
            "--json",
            "--timeout-seconds",
            "30",
        ],
        Some(&environment),
    );
    assert!(
        started_output.status.success(),
        "command failed: {}\nRuntime logs:\n{}",
        String::from_utf8_lossy(&started_output.stderr),
        fixture_runtime_logs(&fixture),
    );
    eprintln!("E2E lifecycle validating runtime start response");
    let started = stdout_json(&started_output);
    assert_complete_envelope(&started, "runtime.start");
    assert_eq!(started["data"]["build"]["cacheHit"], true);
    assert_eq!(started["data"]["runtime"]["schemaVersion"], "2.0.0");
    assert_eq!(started["data"]["runtime"]["mode"], "portable");
    assert_eq!(started["data"]["runtime"]["state"], "ready");
    let session_id = started["runtimeSessionId"]
        .as_str()
        .expect("runtime session ID");
    assert_eq!(started["data"]["runtime"]["sessionId"], session_id);
    let url = started["data"]["runtime"]["url"]
        .as_str()
        .expect("runtime URL");
    assert!(url.starts_with("http://127.0.0.1:"));
    eprintln!("E2E lifecycle probing runtime HTTP endpoint");
    assert_http_ok(url);
    eprintln!("E2E lifecycle observed runtime HTTP readiness");

    for (verb, expected_command) in [
        ("status", "runtime.status"),
        ("wait", "runtime.wait"),
        ("url", "runtime.url"),
    ] {
        let observed = stdout_json(&run_portable(
            &fixture,
            &[
                "runtime",
                verb,
                "--session-id",
                session_id,
                "--json",
                "--timeout-seconds",
                "15",
            ],
            None,
        ));
        assert_complete_envelope(&observed, expected_command);
        assert_eq!(observed["runtimeSessionId"], session_id);
        if verb != "url" {
            assert_eq!(
                observed["data"]["state"],
                "ready",
                "{verb} lost Runtime readiness; logs:\n{}",
                fixture_runtime_logs(&fixture),
            );
        }
    }

    let logs = stdout_json(&run_portable(
        &fixture,
        &[
            "runtime",
            "logs",
            "--session-id",
            session_id,
            "--json",
            "--timeout-seconds",
            "15",
        ],
        None,
    ));
    assert_complete_envelope(&logs, "runtime.logs");
    let serialized_logs = serde_json::to_string(&logs).expect("serialize runtime logs");
    assert!(serialized_logs.contains("[REDACTED]"));
    assert!(!serialized_logs.contains(secret));
    assert!(!serialized_logs.contains(&encoded));

    let stopped = stdout_json(&run_portable(
        &fixture,
        &[
            "runtime",
            "stop",
            "--session-id",
            session_id,
            "--json",
            "--timeout-seconds",
            "30",
        ],
        None,
    ));
    assert_complete_envelope(&stopped, "runtime.stop");
    assert_eq!(stopped["data"]["completed"], true);
    let status = stdout_json(&run_portable(
        &fixture,
        &[
            "runtime",
            "status",
            "--session-id",
            session_id,
            "--json",
            "--timeout-seconds",
            "15",
        ],
        None,
    ));
    assert_eq!(status["data"]["state"], "stopped");
    assert!(
        std::net::TcpStream::connect(url.strip_prefix("http://").expect("runtime authority"))
            .is_err()
    );
    drop(fixture);
    fs::remove_dir_all(&fixture_root).expect("remove stopped Runtime fixture");
}

#[test]
fn real_binary_distinguishes_portable_consistency_build_and_readiness_failures() {
    for (mode, expected) in [
        ("consistency", "consistency_failed"),
        ("build", "runtime_build_failed"),
    ] {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let fixture = portable_fixture(temporary.path(), mode, READY_RUNTIME);
        let project_id = fixture_project_id(&fixture);
        let failed = run_portable(
            &fixture,
            &["runtime", "build", "--project-id", &project_id, "--json"],
            None,
        );
        let error = stderr_json(&failed);
        assert_complete_envelope(&error, "runtime.build");
        assert_eq!(error["error"]["code"], expected);
        assert!(error["error"]["diagnosticRef"]
            .as_str()
            .is_some_and(|value| value.starts_with("artifact_")));
    }

    let temporary = tempfile::tempdir().expect("temporary directory");
    let fixture = portable_fixture(temporary.path(), "success", NEVER_READY_RUNTIME);
    let project_id = fixture_project_id(&fixture);
    let failed = run_portable(
        &fixture,
        &[
            "runtime",
            "start",
            "--project-id",
            &project_id,
            "--json",
            "--timeout-seconds",
            "8",
        ],
        None,
    );
    let error = stderr_json(&failed);
    assert_complete_envelope(&error, "runtime.start");
    assert_eq!(
        error["error"]["code"],
        "runtime_readiness_timeout",
        "Runtime logs:\n{}",
        fixture_runtime_logs(&fixture),
    );
    assert!(error["error"]["diagnosticRef"]
        .as_str()
        .is_some_and(|value| value.starts_with("artifact_")));

    let sessions = fixture
        .config_directory
        .join("isolated-cache")
        .join("portable-runtime")
        .join("sessions");
    let records = fs::read_dir(sessions)
        .expect("runtime sessions")
        .map(|entry| entry.expect("runtime session").path().join("session.json"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    let record: Value =
        serde_json::from_slice(&fs::read(&records[0]).expect("runtime session record"))
            .expect("runtime session JSON");
    assert_eq!(record["state"], "failed");
    assert_eq!(record["failureCode"], "runtime_readiness_timeout");
    assert_process_stops(
        record["runtimePid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .expect("failed Runtime PID"),
    );

    let temporary = tempfile::tempdir().expect("temporary directory");
    let fixture = portable_fixture(temporary.path(), "success", READY_RUNTIME);
    fs::write(
        temporary
            .path()
            .join("workspace/Orders/project-settings.user.json"),
        r#"{"settingsParts":[{"type":"Mendix.Core, Version=11.8.9.0"}]}"#,
    )
    .expect("unsupported version fixture");
    let project_id = fixture_project_id(&fixture);
    let unsupported = run_portable(
        &fixture,
        &["runtime", "build", "--project-id", &project_id, "--json"],
        None,
    );
    assert_eq!(unsupported.status.code(), Some(3));
    let error = stderr_json(&unsupported);
    assert_eq!(error["error"]["code"], "runtime_version_unsupported");
    assert!(error["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("Windows Studio Pro Run Locally")));
}

#[test]
fn real_binary_timeout_terminates_mxbuild_and_its_descendants() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let fixture = portable_fixture(temporary.path(), "hang", READY_RUNTIME);
    let project_id = fixture_project_id(&fixture);
    let timed_out = run_portable(
        &fixture,
        &[
            "runtime",
            "build",
            "--project-id",
            &project_id,
            "--json",
            "--timeout-seconds",
            "15",
        ],
        None,
    );
    let error = stderr_json(&timed_out);
    assert_eq!(error["error"]["code"], "operation_failed");
    for marker in ["mxbuild.pid", "mxbuild-child.pid"] {
        let pid = fs::read_to_string(fixture.toolchain.join(marker))
            .expect("fixture PID marker")
            .parse::<u32>()
            .expect("fixture PID");
        assert_process_stops(pid);
    }
}

#[cfg(unix)]
struct FixtureHttpServer {
    address: std::net::SocketAddr,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl FixtureHttpServer {
    fn start() -> Self {
        use std::sync::atomic::Ordering;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind HTTP fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking HTTP fixture");
        let address = listener.local_addr().expect("HTTP fixture address");
        let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        let worker = thread::spawn(move || {
            while !worker_stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut request = [0_u8; 4096];
                        let _ = stream.read(&mut request);
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nready",
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stopping,
            worker: Some(worker),
        }
    }

    fn port(&self) -> u16 {
        self.address.port()
    }
}

#[cfg(unix)]
impl Drop for FixtureHttpServer {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;

        self.stopping.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.address);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(unix)]
struct WinboatRuntimeFixture {
    _temporary: tempfile::TempDir,
    _guest_server: FixtureHttpServer,
    _runtime_server: FixtureHttpServer,
    config_directory: PathBuf,
    compose_path: PathBuf,
    original_compose: String,
    fake_bin: PathBuf,
    fake_state: PathBuf,
    compose_log: PathBuf,
    runtime_port: u16,
    guest_api_port: u16,
}

#[cfg(unix)]
impl WinboatRuntimeFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("WinBoat Runtime fixture");
        let root = temporary.path();
        let config_directory = root.join("config");
        let workspace = root.join("workspace");
        let fake_bin = root.join("bin");
        let fake_state = root.join("fake-docker");
        fs::create_dir_all(&config_directory).expect("config directory");
        fs::create_dir_all(&workspace).expect("workspace directory");
        fs::create_dir_all(&fake_bin).expect("fake binary directory");
        fs::create_dir_all(&fake_state).expect("fake Docker state");

        let guest_server = FixtureHttpServer::start();
        let runtime_server = FixtureHttpServer::start();
        let guest_api_port = guest_server.port();
        let runtime_port = runtime_server.port();
        let compose_path = root.join("docker-compose.yml");
        let original_compose = format!(
            "services:\n  windows:\n    image: winboat-fixture\n    container_name: MendimaruE2EWinBoat\n    volumes:\n      - winboat-storage:/storage\n      - {}:/shared\n    ports:\n      - 127.0.0.1:47280:7148\n      - 127.0.0.1:47300:3389\nvolumes:\n  winboat-storage: {{}}\n",
            workspace.to_string_lossy()
        );
        fs::write(&compose_path, &original_compose).expect("Compose fixture");

        let mut config = fixture_config(&workspace);
        config.compose_file = compose_path.to_string_lossy().into_owned();
        config.container_name = "MendimaruE2EWinBoat".into();
        config.api_url = format!("http://127.0.0.1:{guest_api_port}");
        config.startup_timeout_seconds = 3;
        fs::write(
            config_directory.join("config.json"),
            serde_json::to_vec_pretty(&config).expect("serialize WinBoat fixture config"),
        )
        .expect("WinBoat fixture config");

        fs::write(fake_state.join("api-port"), guest_api_port.to_string()).expect("fake API port");
        fs::write(fake_state.join("runtime-port"), runtime_port.to_string())
            .expect("fake Runtime port");
        write_fake_docker_inspection(
            &fake_state.join("inspect.json"),
            guest_api_port,
            runtime_port,
            "127.0.0.1",
            "winboat-storage",
        );
        fs::copy(
            fake_state.join("inspect.json"),
            fake_state.join("inspect.good.json"),
        )
        .expect("copy good inspection");
        write_fake_docker_inspection(
            &fake_state.join("inspect.bad-storage.json"),
            guest_api_port,
            runtime_port,
            "127.0.0.1",
            "unexpected-storage",
        );

        let compose_log = fake_state.join("compose.log");
        let docker_script = r#"#!/bin/sh
set -eu
state=__STATE__
compose_log=__COMPOSE_LOG__
command=${1:-}
if [ "$command" = "port" ]; then
  private_port=${3:-}
  case "$private_port" in
    7148/tcp) port=$(cat "$state/api-port") ;;
    3389/tcp) port=47300 ;;
    8080/tcp) port=$(cat "$state/runtime-port") ;;
    *) exit 1 ;;
  esac
  printf '127.0.0.1:%s\n' "$port"
  exit 0
fi
if [ "$command" = "inspect" ]; then
  if [ "${2:-}" = "--format" ]; then
    printf 'USERNAME=fixture\nPASSWORD=fixture\n'
  else
    cat "$state/inspect.json"
  fi
  exit 0
fi
if [ "$command" = "compose" ]; then
  compose_file=
  previous=
  for argument in "$@"; do
    if [ "$previous" = "-f" ]; then compose_file=$argument; fi
    previous=$argument
  done
  printf 'compose-up\n' >> "$compose_log"
  if [ -f "$state/fail-once" ]; then
    rm -f "$state/fail-once"
    printf 'Bind for 127.0.0.1 failed: port is already allocated\n' >&2
    exit 1
  fi
  if [ -f "$state/mutate-storage" ]; then
    if grep -q '127.0.0.1::8080/tcp' "$compose_file"; then
      cp "$state/inspect.bad-storage.json" "$state/inspect.json"
    else
      cp "$state/inspect.good.json" "$state/inspect.json"
    fi
  fi
  exit 0
fi
if [ "$command" = "info" ]; then
  printf '29.0.0\n'
  exit 0
fi
exit 1
"#
        .replace("__STATE__", &shell_literal(&fake_state))
        .replace("__COMPOSE_LOG__", &shell_literal(&compose_log));
        let docker = fake_bin.join("docker");
        fs::write(&docker, docker_script).expect("fake Docker executable");
        make_executable(&docker);

        Self {
            _temporary: temporary,
            _guest_server: guest_server,
            _runtime_server: runtime_server,
            config_directory,
            compose_path,
            original_compose,
            fake_bin,
            fake_state,
            compose_log,
            runtime_port,
            guest_api_port,
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let current_path = std::env::var_os("PATH").unwrap_or_default();
        let paths =
            std::iter::once(self.fake_bin.clone()).chain(std::env::split_paths(&current_path));
        let path = std::env::join_paths(paths).expect("fake Docker PATH");
        run_with_environment(
            &self.config_directory,
            arguments,
            &[("PATH", path.as_os_str())],
        )
    }

    fn set_runtime_binding(&self, port: u16, host_ip: &str, storage: &str) {
        fs::write(self.fake_state.join("runtime-port"), port.to_string())
            .expect("replace fake Runtime port");
        write_fake_docker_inspection(
            &self.fake_state.join("inspect.json"),
            self.guest_api_port,
            port,
            host_ip,
            storage,
        );
    }
}

#[cfg(unix)]
fn shell_literal(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn write_fake_docker_inspection(
    path: &Path,
    api_port: u16,
    runtime_port: u16,
    runtime_host_ip: &str,
    storage_source: &str,
) {
    let inspection = serde_json::json!([{
        "State": { "Status": "running" },
        "Mounts": [
            { "Source": storage_source, "Destination": "/storage" },
            { "Source": "/fixture/workspace", "Destination": "/shared" }
        ],
        "NetworkSettings": {
            "Ports": {
                "3389/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "47300" }],
                "7148/tcp": [{ "HostIp": "127.0.0.1", "HostPort": api_port.to_string() }],
                "8080/tcp": [{ "HostIp": runtime_host_ip, "HostPort": runtime_port.to_string() }]
            }
        }
    }]);
    fs::write(
        path,
        serde_json::to_vec(&inspection).expect("serialize fake Docker inspection"),
    )
    .expect("write fake Docker inspection");
}

#[cfg(unix)]
#[test]
fn real_binary_runs_winboat_runtime_through_loopback_and_restores_compose() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = WinboatRuntimeFixture::new();
    let started = stdout_json(&fixture.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--guest-port",
        "8080",
        "--json",
        "--timeout-seconds",
        "15",
    ]));
    assert_complete_envelope(&started, "runtime.start");
    assert!(started["data"].get("build").is_none());
    assert_eq!(started["data"]["runtime"]["backend"], "linux-winboat");
    assert_eq!(started["data"]["runtime"]["mode"], "studio-run-locally");
    assert_eq!(started["data"]["runtime"]["state"], "ready");
    assert_eq!(started["data"]["runtime"]["httpReady"], true);
    assert_eq!(started["data"]["runtime"]["hostPort"], fixture.runtime_port);
    assert_eq!(started["data"]["runtime"]["guestPort"], 8080);
    assert_eq!(started["data"]["runtime"]["studioState"], "running");
    assert_eq!(
        started["capabilitySnapshot"]["manifest"]["runtimeModes"],
        serde_json::json!(["portable", "studio-run-locally"])
    );
    let session_id = started["runtimeSessionId"]
        .as_str()
        .expect("WinBoat Runtime session");
    let first_url = started["data"]["runtime"]["url"]
        .as_str()
        .expect("WinBoat Runtime URL");
    assert_http_ok(first_url);
    let serialized = serde_json::to_string(&started).expect("serialize start response");
    assert!(!serialized.contains(fixture.compose_path.to_string_lossy().as_ref()));
    assert!(!serialized.contains("winboat-storage"));

    let updated_compose = fs::read_to_string(&fixture.compose_path).expect("updated Compose");
    assert!(updated_compose.contains("127.0.0.1::8080/tcp"));
    assert!(updated_compose.contains("winboat-storage:/storage"));
    assert!(!updated_compose.contains("0.0.0.0"));

    let replacement_runtime = FixtureHttpServer::start();
    fixture.set_runtime_binding(replacement_runtime.port(), "127.0.0.1", "winboat-storage");
    let status = stdout_json(&fixture.run(&[
        "runtime",
        "status",
        "--session-id",
        session_id,
        "--json",
        "--timeout-seconds",
        "15",
    ]));
    assert_eq!(status["data"]["state"], "ready");
    assert_eq!(status["data"]["hostPort"], replacement_runtime.port());
    let replacement_url = status["data"]["url"]
        .as_str()
        .expect("re-detected Runtime URL");
    assert_http_ok(replacement_url);

    for (verb, command) in [("wait", "runtime.wait"), ("url", "runtime.url")] {
        let observed = stdout_json(&fixture.run(&[
            "runtime",
            verb,
            "--session-id",
            session_id,
            "--json",
            "--timeout-seconds",
            "15",
        ]));
        assert_complete_envelope(&observed, command);
        if verb == "url" {
            assert_eq!(observed["data"]["url"], replacement_url);
        } else {
            assert_eq!(observed["data"]["httpReady"], true);
        }
    }

    let logs =
        stdout_json(&fixture.run(&["runtime", "logs", "--session-id", session_id, "--json"]));
    assert!(logs["data"]["entries"]
        .as_array()
        .expect("Runtime log entries")
        .iter()
        .any(|entry| entry
            .as_str()
            .is_some_and(|line| line.contains("guest restart changed"))));
    assert!(!serde_json::to_string(&logs)
        .expect("serialize logs")
        .contains(fixture.compose_path.to_string_lossy().as_ref()));

    let session_directory = fixture
        .config_directory
        .join("isolated-cache/winboat-runtime/sessions")
        .join(session_id);
    assert_eq!(
        fs::metadata(&session_directory)
            .expect("private session directory")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(session_directory.join("compose.original.yml"))
            .expect("private Compose backup")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let stopped = stdout_json(&fixture.run(&[
        "runtime",
        "stop",
        "--session-id",
        session_id,
        "--json",
        "--timeout-seconds",
        "15",
    ]));
    assert_eq!(stopped["data"]["completed"], true);
    assert_eq!(
        fs::read_to_string(&fixture.compose_path).expect("restored Compose"),
        fixture.original_compose
    );
    let stopped_status =
        stdout_json(&fixture.run(&["runtime", "status", "--session-id", session_id, "--json"]));
    assert_eq!(stopped_status["data"]["state"], "stopped");
    assert_eq!(stopped_status["data"]["httpReady"], false);
    assert!(stopped_status["data"].get("url").is_none());
    assert_eq!(
        fs::read_to_string(&fixture.compose_log)
            .expect("Compose invocation log")
            .lines()
            .count(),
        2
    );
}

#[cfg(unix)]
#[test]
fn real_binary_recovers_compose_and_distinguishes_winboat_runtime_failures() {
    let conflict = WinboatRuntimeFixture::new();
    fs::write(conflict.fake_state.join("fail-once"), b"1").expect("port conflict marker");
    let failed = conflict.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        "15",
    ]);
    assert_eq!(
        stderr_json(&failed)["error"]["code"],
        "runtime_port_conflict"
    );
    assert_eq!(
        fs::read_to_string(&conflict.compose_path).expect("conflict rollback"),
        conflict.original_compose
    );

    let public = WinboatRuntimeFixture::new();
    public.set_runtime_binding(public.runtime_port, "0.0.0.0", "winboat-storage");
    let failed = public.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        "15",
    ]);
    assert_eq!(
        stderr_json(&failed)["error"]["code"],
        "runtime_port_forwarding_invalid"
    );
    assert_eq!(
        fs::read_to_string(&public.compose_path).expect("public binding rollback"),
        public.original_compose
    );

    let storage = WinboatRuntimeFixture::new();
    fs::write(storage.fake_state.join("mutate-storage"), b"1").expect("storage mutation marker");
    let failed = storage.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        "15",
    ]);
    assert_eq!(
        stderr_json(&failed)["error"]["code"],
        "runtime_port_forwarding_invalid"
    );
    assert_eq!(
        fs::read_to_string(&storage.compose_path).expect("storage rollback"),
        storage.original_compose
    );
    let restored_inspection: Value = serde_json::from_slice(
        &fs::read(storage.fake_state.join("inspect.json")).expect("restored inspection"),
    )
    .expect("inspection JSON");
    assert_eq!(
        restored_inspection[0]["Mounts"][0]["Source"],
        "winboat-storage"
    );

    let offline = WinboatRuntimeFixture::new();
    fs::write(offline.fake_state.join("api-port"), b"1").expect("offline API port");
    let failed = offline.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        "15",
    ]);
    assert_eq!(
        stderr_json(&failed)["error"]["code"],
        "runtime_guest_offline"
    );
    assert_eq!(
        fs::read_to_string(&offline.compose_path).expect("offline Compose"),
        offline.original_compose
    );
    assert!(!offline.compose_log.exists());
}

#[cfg(unix)]
#[test]
fn real_binary_never_reports_winboat_ready_before_http_responds() {
    let fixture = WinboatRuntimeFixture::new();
    let unavailable_listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve unavailable fixture port");
    let unavailable_port = unavailable_listener
        .local_addr()
        .expect("unavailable fixture address")
        .port();
    drop(unavailable_listener);
    fixture.set_runtime_binding(unavailable_port, "127.0.0.1", "winboat-storage");

    let started = stdout_json(&fixture.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        "15",
    ]));
    assert_eq!(started["data"]["runtime"]["state"], "starting");
    assert_eq!(started["data"]["runtime"]["httpReady"], false);
    assert!(started["data"]["runtime"].get("url").is_none());
    let session_id = started["runtimeSessionId"]
        .as_str()
        .expect("starting WinBoat Runtime session");

    fixture.set_runtime_binding(fixture.runtime_port, "127.0.0.1", "winboat-storage");
    let ready = stdout_json(&fixture.run(&[
        "runtime",
        "wait",
        "--session-id",
        session_id,
        "--json",
        "--timeout-seconds",
        "15",
    ]));
    assert_eq!(ready["data"]["state"], "ready");
    assert_eq!(ready["data"]["httpReady"], true);
    assert_http_ok(ready["data"]["url"].as_str().expect("ready URL"));

    let stopped = fixture.run(&[
        "runtime",
        "stop",
        "--session-id",
        session_id,
        "--json",
        "--timeout-seconds",
        "15",
    ]);
    assert!(stopped.status.success(), "stop delayed Runtime fixture");
}

#[cfg(unix)]
#[test]
fn real_binary_refuses_to_overwrite_a_concurrent_compose_edit_on_stop() {
    let fixture = WinboatRuntimeFixture::new();
    let started = stdout_json(&fixture.run(&[
        "runtime",
        "start",
        "--mode",
        "studio-run-locally",
        "--json",
        "--timeout-seconds",
        "15",
    ]));
    let session_id = started["runtimeSessionId"]
        .as_str()
        .expect("WinBoat Runtime session");
    let mut compose = fs::read_to_string(&fixture.compose_path).expect("managed Compose");
    compose.push_str("# concurrent-user-edit\n");
    fs::write(&fixture.compose_path, &compose).expect("concurrent Compose edit");

    let stopped = fixture.run(&[
        "runtime",
        "stop",
        "--session-id",
        session_id,
        "--json",
        "--timeout-seconds",
        "15",
    ]);
    assert_eq!(
        stderr_json(&stopped)["error"]["code"],
        "runtime_compose_recovery_failed"
    );
    assert!(fs::read_to_string(&fixture.compose_path)
        .expect("preserved concurrent Compose edit")
        .contains("# concurrent-user-edit"));
}

#[cfg(windows)]
#[test]
fn windows_native_rejects_winboat_runtime_mode_without_reading_compose() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let config_directory = temporary.path().join("config");
    let workspace = temporary.path().join("workspace");
    fs::create_dir_all(&config_directory).expect("config directory");
    fs::create_dir_all(&workspace).expect("workspace directory");
    let compose = temporary.path().join("must-not-be-read.yml");
    let marker = b"native adapter boundary";
    fs::write(&compose, marker).expect("Compose boundary marker");
    let mut config = fixture_config(&workspace);
    config.compose_file = compose.to_string_lossy().into_owned();
    fs::write(
        config_directory.join("config.json"),
        serde_json::to_vec_pretty(&config).expect("serialize config"),
    )
    .expect("config fixture");

    let rejected = run(
        &config_directory,
        &["runtime", "start", "--mode", "studio-run-locally", "--json"],
    );
    assert_eq!(stderr_json(&rejected)["error"]["code"], "invalid_request");
    assert_eq!(fs::read(&compose).expect("Compose boundary marker"), marker);
}

fn assert_complete_envelope(document: &Value, command: &str) {
    assert_eq!(document["schemaVersion"], "2.0.0");
    assert_eq!(document["command"], command);
    assert!(document["ok"].is_boolean());
    assert!(document["platform"].is_string());
    assert!(document["backend"].is_string());
    assert!(document["sessionId"]
        .as_str()
        .is_some_and(|value| value.starts_with("session_")));
    assert!(document["capabilitySnapshot"].is_object());
}
