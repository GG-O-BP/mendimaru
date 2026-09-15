//! Real CLI dispatch and keeper processes; only Docker, RDP and guest health
//! are replaced. No live VM, user configuration or credentials are accessed.
use super::*;
use crate::contracts::{ArtifactDescriptor, ArtifactKind};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Instant;

const RUNTIME_ID: &str = "runtime_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const STUDIO_ID: &str = "studio-4242-638908128000000000";
const CHILD_TEST: &str = "cli::runtime_stop_tests::isolated_runtime_stop_process";

struct Process(Child);

impl Process {
    fn finish(&mut self) {
        until(|| self.0.try_wait().unwrap().is_some());
        assert!(self.0.wait().unwrap().success());
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Each fixture process owns its group, including its RDP stand-in.
        // Reap them even when an assertion fails before the disconnect marker.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "Runtime stop fixture timed out");
        thread::sleep(Duration::from_millis(25));
    }
}

struct Fixture {
    root: tempfile::TempDir,
    original: String,
    health: Option<thread::JoinHandle<()>>,
    health_address: std::net::SocketAddr,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        let bin = path.join("bin");
        fs::create_dir(&bin).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let health_address = listener.local_addr().unwrap();
        let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stopping.clone();
        let unready = path.join("http-unready");
        let health = thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = [0; 2048];
                let _ = stream.read(&mut request);
                let response = if unready.exists() && request.starts_with(b"GET / HTTP/") {
                    b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .as_slice()
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .as_slice()
                };
                let _ = stream.write_all(response);
            }
        });
        let original = "services:\n  windows:\n    image: ghcr.io/dockur/windows:e2e-fixture\n    container_name: WinBoat\n    volumes:\n      - fixture-storage:/storage\n    ports:\n      - 127.0.0.1:47280:7148/tcp\nvolumes:\n  fixture-storage: {}\n".to_string();
        let managed = original.replace(
            "    ports:\n",
            "    ports:\n      - 127.0.0.1:8080:8080/tcp\n",
        );
        fs::write(path.join("compose.yml"), &managed).unwrap();
        let mut config = super::tests::app_config(path);
        config.compose_file = path.join("compose.yml").to_string_lossy().into_owned();
        config.api_url = format!("http://{health_address}");
        config.startup_timeout_seconds = 15;
        fs::write(
            path.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let directory = path.join("cache/winboat-runtime/sessions").join(RUNTIME_ID);
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let record = json!({
            "schemaVersion": CONTRACT_SCHEMA_VERSION,
            "sessionId": RUNTIME_ID,
            "backend": "linux-winboat",
            "mode": "studio-run-locally",
            "studioSessionId": STUDIO_ID,
            "studioState": "running",
            "studioProcessId": 4242,
            "state": "ready",
            "httpReady": true,
            "hostPort": 8080,
            "guestPort": 8080,
            "startedAt": "2026-09-08T00:00:00Z",
            "readinessTimeoutSeconds": 15,
            "failureCode": null,
            "logArtifact": ArtifactDescriptor::create(RUNTIME_ID, BackendId::LinuxWinboat, ArtifactKind::RuntimeLog).unwrap(),
            "composeChanged": true,
            "originalComposeSha256": format!("{:x}", Sha256::digest(original.as_bytes())),
            "managedComposeSha256": format!("{:x}", Sha256::digest(managed.as_bytes())),
            "storageMountIdentity": ["fixture-storage"]
        });
        for (name, bytes) in [
            ("session.json", serde_json::to_vec(&record).unwrap()),
            ("compose.original.yml", original.as_bytes().to_vec()),
            ("runtime.log", Vec::new()),
        ] {
            let file = directory.join(name);
            fs::write(&file, bytes).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(path.join("health-port"), health_address.port().to_string()).unwrap();
        let docker = bin.join("docker");
        fs::write(&docker, r#"#!/bin/sh
set -eu
cd "$MENDIMARU_STOP_FIXTURE_ROOT"
case "$1" in
  port) printf '127.0.0.1:%s\n' "$(cat health-port)" ;;
  inspect) printf '[{"State":{"Status":"running"},"Mounts":[{"Source":"fixture-storage","Destination":"/storage"}]}]\n' ;;
  compose)
    if ! mkdir compose-active 2>/dev/null; then
      touch compose-overlap
      exit 1
    fi
    trap 'rmdir compose-active' EXIT
    printf 'compose-up\n' >> compose.log
    touch client-disconnected compose-entered
    while [ -f hold-compose ]; do sleep 0.05; done
    if [ -f fail-once ]; then rm fail-once; exit 1; fi
    printf 'WinBoat running\n' > container-state
    ;;
  *) exit 1 ;;
esac
"#).unwrap();
        fs::set_permissions(docker, fs::Permissions::from_mode(0o700)).unwrap();
        let rdp = bin.join("xfreerdp3");
        fs::write(
            &rdp,
            "#!/bin/sh\ntouch \"$MENDIMARU_STOP_FIXTURE_ROOT/unexpected-rdp\"\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(rdp, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            original,
            health: Some(health),
            health_address,
            stopping,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn spawn(&self, mode: &str) -> Process {
        let path = std::env::join_paths(std::iter::once(self.path("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        Process(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--nocapture"])
                .env("MENDIMARU_STOP_FIXTURE_MODE", mode)
                .env("MENDIMARU_STOP_FIXTURE_ROOT", self.root.path())
                .env("MENDIMARU_CONFIG_DIR", self.root.path())
                .env("MENDIMARU_CACHE_DIR", self.path("cache"))
                .env("PATH", path)
                .stdin(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap(),
        )
    }

    fn calls(&self) -> usize {
        fs::read_to_string(self.path("compose.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn prepare_browser(&self) {
        let record_path = self.path(&format!(
            "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
        ));
        let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        record["hostPort"] = json!(self.health_address.port());
        fs::write(record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        fs::write(self.path("inspect.json"), serde_json::to_vec(&json!([{
            "Id": "unchanged-container-identity",
            "State": {"Status": "running"},
            "Mounts": [{"Source": "fixture-storage", "Destination": "/storage"}],
            "NetworkSettings": {"Ports": {
                "8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": self.health_address.port().to_string()}]
            }}
        }])).unwrap()).unwrap();
        let docker = self.path("bin/docker");
        let script = fs::read_to_string(&docker).unwrap().replace(
            "  inspect) printf",
            "  inspect) cat inspect.json ;;\n  unused-inspect) printf",
        );
        fs::write(docker, script).unwrap();
        fs::write(self.path("smoke.browser.json"), r#"{
            "schemaVersion":"1.0.0", "name":"Keeper linked runtime",
            "beforeEach":[{"action":"goto","path":"/"}],
            "tests":[{"name":"HTTP content", "steps":[{
                "action":"expectText", "locator":{"by":"text","value":"ok","exact":true}, "value":"ok"
            }]}]
        }"#).unwrap();
    }

    fn studio_status(&self) -> crate::contracts::StudioSessionStatus {
        let paths = AppPaths::for_tests(self.root.path().into(), self.path("cache"));
        tauri::async_runtime::block_on(keeper_session(&paths, STUDIO_ID))
            .unwrap()
            .unwrap()
    }

    fn confirm_studio_exit(&self) {
        fs::write(
            self.path("closed.tmp"),
            crate::winboat::keeper_test_stop_report(),
        )
        .unwrap();
        fs::rename(self.path("closed.tmp"), self.path("client-report.json")).unwrap();
    }

    fn result(&self, mode: &str) -> Value {
        serde_json::from_slice(&fs::read(self.path(&format!("{mode}.json"))).unwrap()).unwrap()
    }

    fn assert_stopped(&self, calls: usize) {
        assert!(!self.path("compose-overlap").exists());
        assert_eq!(self.calls(), calls);
        assert_eq!(
            fs::read_to_string(self.path("compose.yml")).unwrap(),
            self.original
        );
        assert_eq!(
            fs::read_to_string(self.path("container-state")).unwrap(),
            "WinBoat running\n"
        );
        let record: Value = serde_json::from_slice(
            &fs::read(self.path(&format!(
                "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
            )))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["state"], "stopped");
        assert_eq!(record["httpReady"], false);
        assert!(record["failureCode"].is_null());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.health_address);
        self.health.take().unwrap().join().unwrap();
    }
}

#[test]
fn isolated_runtime_stop_process() {
    let Ok(mode) = std::env::var("MENDIMARU_STOP_FIXTURE_MODE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("MENDIMARU_STOP_FIXTURE_ROOT").unwrap());
    crate::i18n::initialize("en-US").unwrap();
    if mode == "lock-owner" {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join(".mendimaru-runtime-stop.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&file).unwrap();
        fs::write(root.join("lock-owned"), b"owned").unwrap();
        thread::sleep(Duration::from_secs(60));
    } else if mode == "keeper-prepare-failed" || mode == "keeper-unaccepted" {
        use crate::models::{OperationKind, OperationStage, OperationState};

        let paths = AppPaths::discover_for_cli().unwrap();
        let config = crate::application::load_config(&paths).unwrap();
        preflight_session_keeper(&paths).unwrap();
        let tracker = crate::operations::OperationTracker::begin_with_paths(
            &paths,
            &config,
            OperationKind::Launch,
            "11.12.3",
            true,
            OperationStage::Launching,
            None,
        )
        .unwrap();
        let operation_id = tracker.id().to_string();
        tracker.succeed().unwrap();
        fs::write(
            root.join("client-stop-report.json"),
            crate::winboat::keeper_test_stop_report(),
        )
        .unwrap();
        // The RDP stand-in exits when the real cleanup writes its stop request.
        let client = Command::new("sh")
            .args([
                "-c",
                "while [ ! -f client-control.json ]; do sleep 0.05; done; mv client-stop-report.json client-report.json; touch client-exited",
            ])
            .current_dir(&root)
            .spawn()
            .unwrap();
        crate::winboat::register_keeper_test_client(&config, client);
        let socket = ensure_session_socket_directory(&paths)
            .unwrap()
            .join(session_socket_name(STUDIO_ID));
        if mode == "keeper-prepare-failed" {
            // A path becomes unusable after preflight and a completed launch.
            fs::write(&socket, b"preserve-untrusted-file").unwrap();
        }
        let exit = finish_session_keeper_launch(SessionKeeperResponse {
            ok: true,
            operation_id: Some(operation_id.clone()),
            studio_session_id: Some(STUDIO_ID.into()),
            error: None,
        });
        assert_eq!(exit, EXIT_OPERATION_FAILED);
        assert!(root.join("client-exited").exists());
        assert!(crate::winboat::registered_client_sessions().is_empty());
        let records = crate::operations::list_with_paths(&paths, &config).unwrap();
        let operation = records
            .iter()
            .find(|record| record.id == operation_id)
            .unwrap();
        assert_eq!(operation.state, OperationState::Interrupted);
        assert!(!operation.retryable);
        if mode == "keeper-prepare-failed" {
            assert_eq!(fs::read(&socket).unwrap(), b"preserve-untrusted-file");
            fs::remove_file(&socket).unwrap();
        } else {
            assert!(!socket.exists());
        }
        // Retry can acquire the same session path after the failed handshake.
        let (listener, guard, _) = prepare_session_keeper(STUDIO_ID).unwrap();
        drop(listener);
        drop(guard);
        assert!(!socket.exists());
    } else if mode == "keeper" || mode == "keeper-observe" {
        let config =
            crate::application::load_config(&AppPaths::discover_for_cli().unwrap()).unwrap();
        let client = Command::new("sh")
            .args([
                "-c",
                "while [ ! -f client-disconnected ]; do sleep 0.05; done",
            ])
            .current_dir(&root)
            .spawn()
            .unwrap();
        crate::winboat::register_keeper_test_client(&config, client);
        if mode == "keeper" {
            // #146 exercises concurrent cleanup after a confirmed Studio exit.
            // RDP loss alone must no longer start cleanup (#148).
            fs::write(
                root.join("client-stop-report.json"),
                crate::winboat::keeper_test_stop_report(),
            )
            .unwrap();
            let report_root = root.clone();
            thread::spawn(move || {
                until(|| report_root.join("client-disconnected").exists());
                fs::rename(
                    report_root.join("client-stop-report.json"),
                    report_root.join("client-report.json"),
                )
                .unwrap();
            });
        }
        assert_eq!(crate::winboat::registered_client_sessions().len(), 1);
        let (listener, guard, session_id) = prepare_session_keeper(STUDIO_ID).unwrap();
        fs::write(root.join("keeper-ready"), b"ready").unwrap();
        tauri::async_runtime::block_on(serve_session_keeper(listener, guard, &session_id));
        assert!(crate::winboat::registered_client_sessions().is_empty());
        fs::write(root.join("keeper-finished"), b"finished").unwrap();
    } else if mode == "runtime-reads" {
        for action in ["status", "wait", "url", "logs", "list"] {
            let mut args = vec!["runtime", action, "--json", "--timeout-seconds", "15"];
            if action != "list" {
                args.extend(["--session-id", RUNTIME_ID]);
            }
            let execution =
                execute(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).unwrap();
            assert_eq!(execution.exit_code, EXIT_OK, "{}", execution.stderr);
        }
    } else if mode == "browser" || mode == "browser-local" {
        if mode == "browser-local" {
            let config =
                crate::application::load_config(&AppPaths::discover_for_cli().unwrap()).unwrap();
            let client = Command::new("sleep").arg("60").spawn().unwrap();
            crate::winboat::register_keeper_test_client(&config, client);
        }
        let suite = root.join("smoke.browser.json");
        let execution = execute(
            &[
                "browser",
                "test",
                "--runtime-session-id",
                RUNTIME_ID,
                "--suite-path",
                suite.to_str().unwrap(),
                "--json",
                "--timeout-seconds",
                "15",
            ]
            .map(OsString::from),
        )
        .unwrap();
        let output = if execution.exit_code == EXIT_OK {
            execution.stdout
        } else {
            execution.stderr
        };
        fs::write(root.join(format!("{mode}.json")), output).unwrap();
        if mode == "browser-local" {
            crate::winboat::disconnect_client(STUDIO_ID);
        }
    } else {
        let timeout = if mode == "timeout" { "1" } else { "15" };
        fs::write(root.join(format!("{mode}-started")), b"started").unwrap();
        let execution = execute(
            &[
                "runtime",
                if mode == "runtime-wait" {
                    "wait"
                } else {
                    "stop"
                },
                "--session-id",
                RUNTIME_ID,
                "--json",
                "--timeout-seconds",
                timeout,
            ]
            .map(OsString::from),
        )
        .unwrap();
        let output = if execution.exit_code == EXIT_OK {
            execution.stdout
        } else {
            execution.stderr
        };
        eprintln!("{mode}: {output}");
        fs::write(root.join(format!("{mode}.json")), output).unwrap();
    }
}

#[test]
fn browser_runtime_uses_the_live_keeper_without_replacing_its_client() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let before = fixture.studio_status();
    let compose = fs::read(fixture.path("compose.yml")).unwrap();
    let inspection = fs::read(fixture.path("inspect.json")).unwrap();
    for _ in 0..2 {
        fixture.spawn("browser").finish();
        fixture.spawn("runtime-reads").finish();
        let result = fixture.result("browser");
        assert_eq!(result["data"]["outcome"], "passed", "{result}");
        assert_eq!(result["data"]["passed"], 1);
        assert_eq!(fixture.studio_status(), before);
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        assert_eq!(fs::read(fixture.path("inspect.json")).unwrap(), inspection);
        assert_eq!(fixture.calls(), 0);
        assert!(!fixture.path("unexpected-rdp").exists());
        assert!(keeper.0.try_wait().unwrap().is_none());
    }
    fixture.confirm_studio_exit();
    keeper.finish();
    fixture.assert_stopped(1);
}

#[test]
fn browser_runtime_uses_the_gui_owned_local_session_metadata() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    fixture.spawn("browser-local").finish();
    let result = fixture.result("browser-local");
    assert_eq!(result["data"]["outcome"], "passed", "{result}");
    assert_eq!(fixture.calls(), 0);
    assert!(!fixture.path("unexpected-rdp").exists());
}

#[test]
fn runtime_wait_timeout_keeps_studio_and_reports_unavailable_metadata_as_unknown() {
    for owned in [true, false] {
        let fixture = Fixture::new();
        fixture.prepare_browser();
        fs::write(fixture.path("http-unready"), b"unready").unwrap();
        let record_path = fixture.path(&format!(
            "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
        ));
        let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        record["readinessTimeoutSeconds"] = json!(1);
        fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        let mut keeper = owned.then(|| fixture.spawn("keeper-observe"));
        if owned {
            until(|| fixture.path("keeper-ready").exists());
        }
        let compose = fs::read(fixture.path("compose.yml")).unwrap();
        fixture.spawn("runtime-wait").finish();
        let result = fixture.result("runtime-wait");
        assert_eq!(
            result["error"]["code"], "runtime_readiness_timeout",
            "{result}"
        );
        let record: Value = serde_json::from_slice(&fs::read(record_path).unwrap()).unwrap();
        assert_eq!(
            record["studioState"],
            if owned { "running" } else { "unknown" }
        );
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        assert_eq!(fixture.calls(), 0);
        assert!(!fixture.path("unexpected-rdp").exists());
        if let Some(keeper) = keeper.as_mut() {
            assert!(keeper.0.try_wait().unwrap().is_none());
            fixture.confirm_studio_exit();
            keeper.finish();
            fixture.assert_stopped(1);
        }
    }
}

#[test]
fn disconnected_keeper_preserves_runtime_until_an_authenticated_studio_exit() {
    use crate::contracts::{StudioConnectionState, StudioProcessState};
    let fixture = Fixture::new();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let identity = fixture.studio_status();
    let compose = fs::read(fixture.path("compose.yml")).unwrap();
    fs::write(fixture.path("client-disconnected"), b"disconnect").unwrap();
    until(|| fixture.studio_status().connection == StudioConnectionState::Disconnected);
    for report in [None, Some(b"{\"sessions\":[]}".as_slice())] {
        match report {
            None => fs::remove_file(fixture.path("client-report.json")).unwrap(),
            Some(bytes) => fs::write(fixture.path("client-report.json"), bytes).unwrap(),
        }
        // Include the keeper's automatic timer, not only on-demand status.
        thread::sleep(Duration::from_millis(1_200));
        let status = fixture.studio_status();
        assert_eq!(status.state, StudioProcessState::Unknown);
        assert_eq!(status.process_id, identity.process_id);
        assert_eq!(status.started_at, identity.started_at);
        assert_eq!(fixture.calls(), 0);
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        assert!(keeper.0.try_wait().unwrap().is_none());
    }
    fixture.confirm_studio_exit();
    keeper.finish();
    fixture.assert_stopped(1);
}

#[test]
fn browser_metadata_failures_are_bounded_diagnostic_and_never_open_rdp() {
    use std::os::unix::net::UnixListener;
    for response in [
        "absent",
        "timeout",
        "invalid",
        "wrong-id",
        "wrong-schema",
        "wrong-version",
        "stopped",
    ] {
        let fixture = Fixture::new();
        fixture.prepare_browser();
        let paths = AppPaths::for_tests(fixture.root.path().into(), fixture.path("cache"));
        let directory = ensure_session_socket_directory(&paths).unwrap();
        let server = if response == "absent" {
            None
        } else {
            let listener =
                UnixListener::bind(directory.join(session_socket_name(STUDIO_ID))).unwrap();
            Some(thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = stream.read(&mut [0; 32]);
                if response == "timeout" {
                    thread::sleep(Duration::from_secs(3));
                } else if response == "invalid" {
                    let _ = stream.write_all(b"invalid /private/secret\n");
                } else {
                    let mut session = json!({
                        "schemaVersion": CONTRACT_SCHEMA_VERSION, "sessionId": STUDIO_ID,
                        "version": "11.12.3", "state": "running", "processId": 4242,
                        "startedAt": "2025-08-15T00:00:00Z", "connection": "connected", "reconnectable": false
                    });
                    match response {
                        "wrong-id" => {
                            session["sessionId"] = json!("studio-9000-638908128000000000")
                        }
                        "wrong-schema" => session["schemaVersion"] = json!("3.0.0"),
                        "wrong-version" => session["version"] = json!("/private/secret"),
                        "stopped" => session["state"] = json!("stopped"),
                        _ => unreachable!(),
                    }
                    let mut bytes =
                        serde_json::to_vec(&json!({"ok":true, "session":session})).unwrap();
                    bytes.push(b'\n');
                    let _ = stream.write_all(&bytes);
                }
            }))
        };
        let started = Instant::now();
        fixture.spawn("browser").finish();
        assert!(started.elapsed() < Duration::from_secs(8), "{response}");
        if let Some(server) = server {
            server.join().unwrap();
        }
        let result = fixture.result("browser");
        assert_eq!(
            result["error"]["code"], "precondition_failed",
            "{response}: {result}"
        );
        assert_eq!(result["error"]["capability"], "browser.test");
        assert_eq!(result["error"]["retryable"], true);
        assert_eq!(
            result["error"]["message"],
            crate::application::BROWSER_STUDIO_METADATA_UNAVAILABLE
        );
        assert_eq!(fixture.calls(), 0);
        assert!(!fixture.path("unexpected-rdp").exists());
    }
}

#[test]
fn failed_keeper_preparation_and_unaccepted_launch_clean_up_runtime_and_operation() {
    for mode in ["keeper-prepare-failed", "keeper-unaccepted"] {
        let fixture = Fixture::new();
        fixture.spawn(mode).finish();
        fixture.assert_stopped(1);
    }
}

#[test]
fn runtime_stop_and_live_keeper_cleanup_share_one_compose_recreation() {
    for fail_once in [false, true] {
        let fixture = Fixture::new();
        fs::write(fixture.path("hold-compose"), b"hold").unwrap();
        if fail_once {
            fs::write(fixture.path("fail-once"), b"fail").unwrap();
        }
        let mut keeper = fixture.spawn("keeper");
        until(|| fixture.path("keeper-ready").exists());
        let mut stop = fixture.spawn("stop");
        until(|| fixture.path("compose-entered").exists());
        // The Compose operation drops the registered RDP stand-in. Allow the
        // real keeper's one-second polling loop to enter automatic cleanup.
        thread::sleep(Duration::from_millis(1_500));
        assert!(!fixture.path("keeper-finished").exists());
        assert!(!fixture.path("compose-overlap").exists());
        assert_eq!(fixture.calls(), 1);
        fs::remove_file(fixture.path("hold-compose")).unwrap();
        stop.finish();
        keeper.finish();
        let result = fixture.result("stop");
        if fail_once {
            assert_eq!(result["error"]["code"], "runtime_compose_recovery_failed");
            assert_eq!(result["error"]["retryable"], true);
        } else {
            assert_eq!(result["data"]["completed"], true);
        }
        fixture.assert_stopped(if fail_once { 2 } else { 1 });
        // A later explicit retry must return the same final state without
        // recreating the VM again, including after the keeper recovered failure.
        fixture.spawn("retry").finish();
        assert_eq!(fixture.result("retry")["data"]["completed"], true);
        fixture.assert_stopped(if fail_once { 2 } else { 1 });
    }
}

#[test]
fn concurrent_runtime_stop_and_cancelled_waiter_do_not_recreate_twice() {
    let fixture = Fixture::new();
    fs::write(fixture.path("hold-compose"), b"hold").unwrap();
    let mut first = fixture.spawn("first");
    until(|| fixture.path("compose-entered").exists());
    let mut second = fixture.spawn("second");
    let mut timeout = fixture.spawn("timeout");
    timeout.finish();
    assert_eq!(fixture.result("timeout")["ok"], false);
    assert!(!fixture.path("second.json").exists());
    assert_eq!(fixture.calls(), 1);
    fs::remove_file(fixture.path("hold-compose")).unwrap();
    first.finish();
    second.finish();
    for mode in ["first", "second"] {
        assert_eq!(fixture.result(mode)["data"]["completed"], true);
    }
    fixture.assert_stopped(1);
}

#[test]
fn runtime_stop_recovers_after_lock_owner_process_exits() {
    let fixture = Fixture::new();
    let owner = fixture.spawn("lock-owner");
    until(|| fixture.path("lock-owned").exists());
    let mut stop = fixture.spawn("stop");
    until(|| fixture.path("stop-started").exists());
    thread::sleep(Duration::from_millis(100));
    assert_eq!(fixture.calls(), 0);
    drop(owner);
    stop.finish();
    assert_eq!(fixture.result("stop")["data"]["completed"], true);
    fixture.assert_stopped(1);
}

#[test]
fn runtime_stop_refuses_untrusted_lock_files_without_touching_compose() {
    use std::os::unix::fs::symlink;
    for kind in ["symlink", "hardlink", "public", "directory"] {
        let fixture = Fixture::new();
        let lock = fixture.path(".mendimaru-runtime-stop.lock");
        let target = fixture.path("lock-target");
        fs::write(&target, b"untouched").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        match kind {
            "symlink" => symlink(&target, &lock).unwrap(),
            "hardlink" => fs::hard_link(&target, &lock).unwrap(),
            "public" => {
                fs::write(&lock, b"").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
            }
            "directory" => fs::create_dir(&lock).unwrap(),
            _ => unreachable!(),
        }
        let compose = fs::read(fixture.path("compose.yml")).unwrap();
        fixture.spawn("stop").finish();
        let result = fixture.result("stop");
        assert_eq!(result["error"]["code"], "precondition_failed", "{kind}");
        assert_eq!(result["error"]["capability"], "runtime.stop", "{kind}");
        assert_eq!(fixture.calls(), 0);
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert!(!result
            .to_string()
            .contains(fixture.root.path().to_str().unwrap()));
    }
}
