//! Shared browser session (#151) process tests: real CLI dispatch, real
//! Chromium participants, real kernel-held participation. Docker, RDP, guest
//! health and the keeper are fixtures; this is not an actual-VM claim.
use super::*;
use crate::contracts::{ArtifactDescriptor, ArtifactKind};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Instant;

const RUNTIME_ID: &str = "runtime_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const STUDIO_ID: &str = "studio-4242-638908128000000000";
const CHILD_TEST: &str = "cli::browser_session_tests::shared_session_process";

struct Process(Child);

impl Process {
    fn finish(&mut self) {
        until(|| self.0.try_wait().unwrap().is_some());
        assert!(self.0.wait().unwrap().success());
    }

    fn kill(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[track_caller]
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "shared session fixture timed out at line {}",
            std::panic::Location::caller().line()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

struct Fixture {
    root: tempfile::TempDir,
    original: String,
    managed: String,
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
        let browser_hold = path.join("browser-held");
        let browser_release = path.join("browser-release");
        let health = thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = [0; 2048];
                let _ = stream.read(&mut request);
                if request.starts_with(b"GET /hold HTTP/") {
                    fs::write(&browser_hold, b"held").unwrap();
                    while !browser_release.exists()
                        && !stopped.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                    // The server owns barrier hygiene: once released, the
                    // markers clear only after this handler resumes.
                    let _ = fs::remove_file(&browser_hold);
                    let _ = fs::remove_file(&browser_release);
                }
                let response = if unready.exists() && request.starts_with(b"GET / HTTP/") {
                    b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r"
                        .as_slice()
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .as_slice()
                };
                let _ = stream.write_all(response);
            }
        });
        let vm_name = crate::contracts::secure_identifier("vm").unwrap();
        let original = "services:\n  windows:\n    image: ghcr.io/dockur/windows:e2e-fixture\n    labels:\n      io.winboat.managed: 'true'\n    container_name: WinBoat\n    volumes:\n      - fixture-storage:/storage\n    ports:\n      - 127.0.0.1:47280:7148/tcp\nvolumes:\n  fixture-storage: {}\n".replace("container_name: WinBoat", &format!("container_name: {vm_name}"));
        let managed = original.replace(
            "    ports:\n",
            "    ports:\n      - 127.0.0.1:8080:8080/tcp\n",
        );
        fs::write(path.join("compose.yml"), &managed).unwrap();
        let mut config = super::tests::app_config(path);
        config.container_name = vm_name;
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
cd "$MENDIMARU_SESSION_FIXTURE_ROOT"
case "$1" in
  port) printf '127.0.0.1:%s\n' "$(cat health-port)" ;;
  inspect)
    if [ "$2" = --format ]; then python3 -c 'import json; d=json.load(open("inspect.json"))[0]; print(json.dumps(dict(id=d["Id"],running=d["State"]["Running"],ports=d["NetworkSettings"]["Ports"])))'; else cat inspect.json; fi ;;
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
            "#!/bin/sh\ntouch \"$MENDIMARU_SESSION_FIXTURE_ROOT/unexpected-rdp\"\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(rdp, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            original,
            managed,
            health: Some(health),
            health_address,
            stopping,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn spawn(&self, mode: &str) -> Process {
        self.spawn_with(mode, None)
    }

    fn spawn_with(&self, mode: &str, shared_id: Option<&str>) -> Process {
        let path = std::env::join_paths(std::iter::once(self.path("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env("MENDIMARU_SESSION_FIXTURE_MODE", mode)
            .env("MENDIMARU_SESSION_FIXTURE_ROOT", self.root.path())
            .env("MENDIMARU_CONFIG_DIR", self.root.path())
            .env("MENDIMARU_CACHE_DIR", self.path("cache"))
            .env("PATH", path)
            .stdin(Stdio::null())
            .process_group(0);
        if let Some(shared_id) = shared_id {
            command.env("MENDIMARU_SHARED_SESSION_ID", shared_id);
        }
        Process(command.spawn().unwrap())
    }

    fn prepare_browser(&self) {
        let record_path = self.path(&format!(
            "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
        ));
        let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        record["hostPort"] = json!(self.health_address.port());
        fs::write(record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        fs::write(
            self.path("inspect.json"),
            serde_json::to_vec(&json!([{
                "Id": "b".repeat(64),
                "State": {"Status": "running", "Running": true},
                "Mounts": [{"Source": "fixture-storage", "Destination": "/storage"}],
                "NetworkSettings": {"Ports": {
                    "8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": self.health_address.port().to_string()}]
                }}
            }]))
            .unwrap(),
        )
        .unwrap();
        for (name, body) in [
            (
                "smoke.browser.json",
                r#"{
            "schemaVersion":"1.0.0", "name":"Shared session participant",
            "beforeEach":[{"action":"goto","path":"/"}],
            "tests":[{"name":"HTTP content", "steps":[{
                "action":"expectText", "locator":{"by":"text","value":"ok","exact":true}, "value":"ok"
            }]}]
        }"#,
            ),
            (
                "hold.browser.json",
                r#"{
            "schemaVersion":"1.0.0", "name":"Held participant",
            "beforeEach":[{"action":"goto","path":"/hold"}],
            "tests":[{"name":"Held", "steps":[{
                "action":"expectText", "locator":{"by":"text","value":"ok","exact":true}, "value":"ok"
            }]}]
        }"#,
            ),
        ] {
            fs::write(self.path(name), body).unwrap();
        }
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

    fn calls(&self) -> usize {
        fs::read_to_string(self.path("compose.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn assert_preserved(&self) {
        assert_eq!(
            fs::read(self.path("compose.yml")).unwrap(),
            self.managed.as_bytes()
        );
        assert_eq!(self.calls(), 0);
        assert!(!self.path("unexpected-rdp").exists());
        let record: Value = serde_json::from_slice(
            &fs::read(self.path(&format!(
                "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
            )))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["state"], "ready");
        assert_eq!(record["httpReady"], true);
    }

    fn assert_stopped_exactly_once(&self) {
        assert!(!self.path("compose-overlap").exists());
        assert_eq!(self.calls(), 1);
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
fn shared_session_process() {
    let Ok(mode) = std::env::var("MENDIMARU_SESSION_FIXTURE_MODE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("MENDIMARU_SESSION_FIXTURE_ROOT").unwrap());
    crate::i18n::initialize("en-US").unwrap();
    let shared_id = std::env::var("MENDIMARU_SHARED_SESSION_ID").ok();
    let suite = |name: &str| root.join(name).to_string_lossy().into_owned();
    let run = |args: Vec<String>| -> CliExecution {
        let arguments = args.into_iter().map(OsString::from).collect::<Vec<_>>();
        execute(&arguments).expect("shared session child execution")
    };
    let output = |name: &str, execution: &CliExecution| {
        let bytes = if execution.stdout.is_empty() {
            execution.stderr.as_bytes()
        } else {
            execution.stdout.as_bytes()
        };
        fs::write(root.join(format!("{name}.json")), bytes).unwrap();
    };
    match mode.as_str() {
        "keeper-observe" => {
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
            assert_eq!(crate::winboat::registered_client_sessions().len(), 1);
            let (listener, guard, session_id) = prepare_session_keeper(STUDIO_ID).unwrap();
            fs::write(root.join("keeper-ready"), b"ready").unwrap();
            tauri::async_runtime::block_on(serve_session_keeper(listener, guard, &session_id));
            assert!(crate::winboat::registered_client_sessions().is_empty());
            fs::write(root.join("keeper-finished"), b"finished").unwrap();
        }
        "session-prepare" => {
            let mut args = vec![
                "browser".into(),
                "session".into(),
                "prepare".into(),
                "--runtime-session-id".into(),
                RUNTIME_ID.into(),
                "--json".into(),
                "--timeout-seconds".into(),
                "20".into(),
            ];
            if std::env::var_os("MENDIMARU_SESSION_OWNS").is_some() {
                args.push("--owns-runtime".into());
                args.push("--finalize-policy".into());
                args.push("stop".into());
            }
            let execution = run(args);
            output("session-prepare", &execution);
            assert_eq!(execution.exit_code, EXIT_OK, "{}", execution.stderr);
        }
        "session-prepare-unready" => {
            let execution = run(vec![
                "browser".into(),
                "session".into(),
                "prepare".into(),
                "--runtime-session-id".into(),
                RUNTIME_ID.into(),
                "--json".into(),
                "--timeout-seconds".into(),
                "20".into(),
            ]);
            output("session-prepare-unready", &execution);
            assert_eq!(execution.exit_code, EXIT_OPERATION_FAILED);
        }
        "session-participant" | "session-participant-hold" => {
            let held = mode == "session-participant-hold";
            let args = vec![
                "browser".into(),
                "test".into(),
                "--shared-session-id".into(),
                shared_id.expect("shared session id"),
                "--suite-path".into(),
                suite(if held {
                    "hold.browser.json"
                } else {
                    "smoke.browser.json"
                }),
                "--json".into(),
                "--timeout-seconds".into(),
                "60".into(),
            ];
            if held {
                fs::write(root.join("participant-started"), b"started").unwrap();
            }
            let execution = run(args);
            output("session-participant", &execution);
        }
        "session-status" => {
            let execution = run(vec![
                "browser".into(),
                "session".into(),
                "status".into(),
                "--shared-session-id".into(),
                shared_id.expect("shared session id"),
                "--json".into(),
                "--timeout-seconds".into(),
                "10".into(),
            ]);
            output("session-status", &execution);
            assert_eq!(execution.exit_code, EXIT_OK, "{}", execution.stderr);
        }
        "session-finalize" => {
            let mut args = vec![
                "browser".into(),
                "session".into(),
                "finalize".into(),
                "--shared-session-id".into(),
                shared_id.expect("shared session id"),
                "--json".into(),
                "--timeout-seconds".into(),
                "60".into(),
            ];
            if let Some(timeout) = std::env::var_os("MENDIMARU_SESSION_DRAIN_MS") {
                args.push("--timeout-ms".into());
                args.push(timeout.to_string_lossy().into_owned());
            }
            let execution = run(args);
            output("session-finalize", &execution);
        }
        "runtime-stop" => {
            let execution = run(vec![
                "runtime".into(),
                "stop".into(),
                "--session-id".into(),
                RUNTIME_ID.into(),
                "--json".into(),
                "--timeout-seconds".into(),
                "10".into(),
            ]);
            output("runtime-stop", &execution);
        }
        "vm-exclusive" => {
            let config =
                crate::application::load_config(&AppPaths::discover_for_cli().unwrap()).unwrap();
            let lease = tauri::async_runtime::block_on(crate::winboat::vm_use::acquire(
                &config,
                crate::winboat::vm_use::Mode::Exclusive,
                crate::contracts::CapabilityId::RuntimeStop,
            ))
            .unwrap();
            std::mem::forget(lease);
            fs::write(root.join("vm-exclusive"), b"held").unwrap();
            thread::sleep(Duration::from_secs(60));
        }
        _ => panic!("unknown shared session fixture mode {mode}"),
    }
}

fn prepared_session(fixture: &Fixture, keeper: &mut Process) -> (Value, String) {
    fixture.spawn("session-prepare").finish();
    let prepared = fixture.result("session-prepare");
    assert_eq!(prepared["ok"], true, "{prepared}");
    let descriptor = &prepared["data"];
    assert_eq!(descriptor["state"], "ready");
    assert_eq!(descriptor["runtimeSessionId"], RUNTIME_ID);
    assert_eq!(descriptor["finalizePolicy"], "keep");
    assert_eq!(descriptor["runtimeOrigin"], "attached-existing");
    assert!(
        descriptor["identity"]["baseUrl"]
            .as_str()
            .is_some_and(|url| url.contains(&fixture.health_address.port().to_string())),
        "{prepared}"
    );
    assert_eq!(descriptor["identity"]["studioSessionId"], STUDIO_ID);
    let shared_id = descriptor["sessionId"].as_str().unwrap().to_string();
    assert!(keeper.0.try_wait().unwrap().is_none());
    (prepared, shared_id)
}

fn status(fixture: &Fixture, shared_id: &str) -> Value {
    fixture
        .spawn_with("session-status", Some(shared_id))
        .finish();
    let status = fixture.result("session-status");
    assert_eq!(status["ok"], true, "{status}");
    status["data"].clone()
}

#[test]
fn two_workers_join_one_prepared_app_and_finalize_keeps_the_runtime() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let before = fixture.studio_status();
    let (_, shared_id) = prepared_session(&fixture, &mut keeper);

    let mut first = fixture.spawn_with("session-participant", Some(&shared_id));
    let mut second = fixture.spawn_with("session-participant", Some(&shared_id));
    first.finish();
    second.finish();
    let first_result = fixture.result("session-participant");
    assert_eq!(first_result["data"]["outcome"], "passed", "{first_result}");
    assert_eq!(first_result["data"]["passed"], 1);

    let report = status(&fixture, &shared_id);
    assert_eq!(report["liveParticipants"], 0);
    assert_eq!(report["state"], "ready");

    fixture
        .spawn_with("session-finalize", Some(&shared_id))
        .finish();
    let finalized = fixture.result("session-finalize");
    assert_eq!(finalized["data"]["finalized"], true, "{finalized}");
    assert_eq!(finalized["data"]["cleanup"], "none");
    assert_eq!(status(&fixture, &shared_id)["state"], "finalized");

    // Attach after finalize is an explicit refusal, not a silent reuse.
    let mut late = fixture.spawn_with("session-participant", Some(&shared_id));
    until(|| late.0.try_wait().unwrap().is_some());
    let late_output = fixture.result("session-participant");
    assert_eq!(late_output["error"]["code"], "precondition_failed");
    assert!(late_output["error"]["message"]
        .as_str()
        .unwrap()
        .contains("finalized"));

    fixture.assert_preserved();
    assert_eq!(fixture.studio_status(), before);
    assert!(keeper.0.try_wait().unwrap().is_none());
    keeper.kill();
}

#[test]
fn crashed_worker_keeps_identity_and_the_last_finalize_cleans_once() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let (_, shared_id) = prepared_session(&fixture, &mut keeper);

    let mut held = fixture.spawn_with("session-participant-hold", Some(&shared_id));
    until(|| fixture.path("participant-started").exists());
    until(|| fixture.path("browser-held").exists());

    // The held worker crashes mid-navigation: the kernel drops its lock while
    // the Runtime identity stays intact for the other worker.
    held.kill();
    fs::write(fixture.path("browser-release"), b"").unwrap();
    until(|| !fixture.path("browser-held").exists());
    let mut other = fixture.spawn_with("session-participant", Some(&shared_id));
    other.finish();
    let other_result = fixture.result("session-participant");
    assert_eq!(other_result["data"]["outcome"], "passed", "{other_result}");
    until(|| status(&fixture, &shared_id)["liveParticipants"] == json!(0));
    let report = status(&fixture, &shared_id);
    assert_eq!(report["state"], "ready");
    assert_eq!(report["runtimeSessionId"], RUNTIME_ID);
    fixture.assert_preserved();
    assert!(keeper.0.try_wait().unwrap().is_none());

    // A live worker blocks finalize; a drain timeout is retryable and leaves
    // the session attachable again, and attach during finalizing is refused.
    until(|| !fixture.path("browser-held").exists());
    let mut blocker = fixture.spawn_with("session-participant-hold", Some(&shared_id));
    until(|| fixture.path("browser-held").exists());
    let mut draining = Command::new(std::env::current_exe().unwrap());
    let path = std::env::join_paths(std::iter::once(fixture.path("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    draining
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env("MENDIMARU_SESSION_FIXTURE_MODE", "session-finalize")
        .env("MENDIMARU_SESSION_FIXTURE_ROOT", fixture.root.path())
        .env("MENDIMARU_SHARED_SESSION_ID", &shared_id)
        .env("MENDIMARU_SESSION_DRAIN_MS", "1500")
        .env("MENDIMARU_CONFIG_DIR", fixture.root.path())
        .env("MENDIMARU_CACHE_DIR", fixture.path("cache"))
        .env("PATH", path)
        .stdin(Stdio::null())
        .process_group(0);
    let mut draining_process = Process(draining.spawn().unwrap());
    // Deterministic ordering: wait until finalize actually holds the
    // finalizing state before the attach attempt races it.
    until(|| status(&fixture, &shared_id)["state"] == json!("finalizing"));
    let mut racing = fixture.spawn_with("session-participant", Some(&shared_id));
    until(|| racing.0.try_wait().unwrap().is_some());
    let racing_output = fixture.result("session-participant");
    assert!(
        racing_output["error"]["message"]
            .as_str()
            .unwrap()
            .contains("finalizing"),
        "{racing_output}"
    );
    draining_process.finish();
    let drained = fixture.result("session-finalize");
    assert_eq!(drained["ok"], false, "{drained}");
    assert_eq!(drained["error"]["code"], "precondition_failed");
    assert_eq!(drained["error"]["retryable"], true);
    assert_eq!(status(&fixture, &shared_id)["state"], "ready");

    // The blocking worker crashes; the retried finalize then completes.
    blocker.kill();
    fs::write(fixture.path("browser-release"), b"").unwrap();
    until(|| !fixture.path("browser-held").exists());
    fixture
        .spawn_with("session-finalize", Some(&shared_id))
        .finish();
    let finalized = fixture.result("session-finalize");
    assert_eq!(finalized["data"]["finalized"], true);
    assert_eq!(finalized["data"]["cleanup"], "none");

    // A duplicate finalize is idempotent and performs no further work.
    fixture
        .spawn_with("session-finalize", Some(&shared_id))
        .finish();
    let duplicate = fixture.result("session-finalize");
    assert_eq!(duplicate["data"]["alreadyFinalized"], true);
    fixture.assert_preserved();
    keeper.kill();
}

#[test]
fn owned_runtime_stops_exactly_once_and_attached_sessions_never_stop() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());

    // An owner-claimed session stops the Runtime exactly once at finalize.
    let mut owner = Command::new(std::env::current_exe().unwrap());
    let path = std::env::join_paths(std::iter::once(fixture.path("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    owner
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env("MENDIMARU_SESSION_FIXTURE_MODE", "session-prepare")
        .env("MENDIMARU_SESSION_FIXTURE_ROOT", fixture.root.path())
        .env("MENDIMARU_SESSION_OWNS", "1")
        .env("MENDIMARU_CONFIG_DIR", fixture.root.path())
        .env("MENDIMARU_CACHE_DIR", fixture.path("cache"))
        .env("PATH", path)
        .stdin(Stdio::null())
        .process_group(0);
    let mut owner_process = Process(owner.spawn().unwrap());
    owner_process.finish();
    let prepared = fixture.result("session-prepare");
    assert_eq!(prepared["data"]["finalizePolicy"], "stop");
    assert_eq!(prepared["data"]["runtimeOrigin"], "owner-started");
    let shared_id = prepared["data"]["sessionId"].as_str().unwrap().to_string();

    let mut participant = fixture.spawn_with("session-participant", Some(&shared_id));
    participant.finish();
    assert_eq!(
        fixture.result("session-participant")["data"]["outcome"],
        "passed"
    );

    fixture.confirm_studio_exit();
    fixture
        .spawn_with("session-finalize", Some(&shared_id))
        .finish();
    let finalized = fixture.result("session-finalize");
    assert_eq!(finalized["data"]["finalized"], true, "{finalized}");
    assert_eq!(finalized["data"]["cleanup"], "runtime-stopped");
    fixture.assert_stopped_exactly_once();

    // Duplicate finalize after the real stop stays a no-op.
    fixture
        .spawn_with("session-finalize", Some(&shared_id))
        .finish();
    let duplicate = fixture.result("session-finalize");
    assert_eq!(duplicate["data"]["alreadyFinalized"], true);
    assert_eq!(fixture.calls(), 1);
    keeper.kill();
}

#[test]
fn lifecycle_exclusion_blocks_participants_in_both_directions() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let (_, shared_id) = prepared_session(&fixture, &mut keeper);

    // An exclusive VM reservation refuses new worker participation.
    let mut exclusive = fixture.spawn("vm-exclusive");
    until(|| fixture.path("vm-exclusive").exists());
    let mut blocked = fixture.spawn_with("session-participant", Some(&shared_id));
    until(|| blocked.0.try_wait().unwrap().is_some());
    let blocked_output = fixture.result("session-participant");
    assert_eq!(blocked_output["error"]["code"], "precondition_failed");
    assert_eq!(blocked_output["error"]["retryable"], true);
    exclusive.kill();

    // A live participant keeps lifecycle mutations busy.
    let mut held = fixture.spawn_with("session-participant-hold", Some(&shared_id));
    until(|| fixture.path("browser-held").exists());
    let mut stopping = fixture.spawn("runtime-stop");
    until(|| stopping.0.try_wait().unwrap().is_some());
    let stop_output = fixture.result("runtime-stop");
    assert_eq!(stop_output["error"]["code"], "precondition_failed");
    assert_eq!(stop_output["error"]["retryable"], true);
    fixture.assert_preserved();
    assert!(keeper.0.try_wait().unwrap().is_none());

    fs::write(fixture.path("browser-release"), b"").unwrap();
    until(|| !fixture.path("browser-held").exists());
    held.finish();
    assert_eq!(
        fixture.result("session-participant")["data"]["outcome"],
        "passed"
    );
    keeper.kill();
}

#[test]
fn unready_runtime_fails_prepare_without_leaving_a_session() {
    let fixture = Fixture::new();
    fixture.prepare_browser();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let registry = format!("/tmp/mendimaru-test-sessions-{}", unsafe {
        libc::geteuid()
    });
    let before = fs::read_dir(&registry)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    fs::write(fixture.path("http-unready"), b"").unwrap();
    fixture.spawn("session-prepare-unready").finish();
    let failed = fixture.result("session-prepare-unready");
    assert_eq!(failed["ok"], false, "{failed}");
    assert_eq!(failed["error"]["code"], "precondition_failed");
    assert_eq!(failed["error"]["retryable"], true);
    // No preparing record is left behind for participants to observe.
    let after = fs::read_dir(&registry)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(after, before);
    keeper.kill();
}
