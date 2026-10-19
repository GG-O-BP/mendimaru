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
        self.finish_with_timeout(Duration::from_secs(15));
    }

    fn finish_with_timeout(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "fixture process timed out");
            thread::sleep(Duration::from_millis(25));
        }
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
        let browser_hold = path.join("browser-held");
        let frontend_hold = path.join("frontend-hold");
        let browser_release = path.join("browser-release");
        let health = thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = [0; 2048];
                let _ = stream.read(&mut request);
                if request.starts_with(&[3, 0, 0, 19]) {
                    let _ = stream.write_all(&[
                        3, 0, 0, 19, 14, 208, 0, 0, 18, 52, 0, 2, 31, 8, 0, 1, 0, 0, 0,
                    ]);
                    continue;
                }
                if request.starts_with(b"GET /hold HTTP/")
                    || (frontend_hold.exists()
                        && String::from_utf8_lossy(&request).contains("HeadlessChrome"))
                {
                    fs::write(&browser_hold, b"held").unwrap();
                    while !browser_release.exists()
                        && !stopped.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
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
                .env("XDG_CONFIG_HOME", self.path("xdg-config"))
                .env("PATH", path)
                .stdin(if mode == "keeper-readiness-failed" {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
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
            "Id": "a".repeat(64),
            "State": {"Status": "running", "Running": true},
            "Mounts": [{"Source": "fixture-storage", "Destination": "/storage"}],
            "NetworkSettings": {"Ports": {
                "8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": self.health_address.port().to_string()}]
            }}
        }])).unwrap()).unwrap();
        let docker = self.path("bin/docker");
        let script = fs::read_to_string(&docker).unwrap().replace(
            "  inspect) printf",
            "  inspect) if [ \"$2\" = --format ]; then python3 -c 'import json; d=json.load(open(\"inspect.json\"))[0]; print(json.dumps(dict(id=d[\"Id\"],running=d[\"State\"][\"Running\"],ports=d[\"NetworkSettings\"][\"Ports\"])))'; else cat inspect.json; fi ;;\n  unused-inspect) printf",
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

    fn prepare_launch_recovery(&self, outcome: &str) {
        self.prepare_browser();
        let mut config: crate::models::AppConfig =
            serde_json::from_slice(&fs::read(self.path("config.json")).unwrap()).unwrap();
        config.freerdp_binary = self.path("bin/xfreerdp3").to_string_lossy().into_owned();
        fs::write(
            self.path("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let mut inspection: Value =
            serde_json::from_slice(&fs::read(self.path("inspect.json")).unwrap()).unwrap();
        for port in ["3389/tcp", "7148/tcp"] {
            inspection[0]["NetworkSettings"]["Ports"][port] = json!([{
                "HostIp": "127.0.0.1", "HostPort": self.health_address.port().to_string()
            }]);
        }
        fs::write(
            self.path("inspect.json"),
            serde_json::to_vec(&inspection).unwrap(),
        )
        .unwrap();
        let docker = self.path("bin/docker");
        let script = fs::read_to_string(&docker).unwrap().replace(
            "if [ \"$2\" = --format ]; then python3",
            "if [ \"$2\" = --format ] && [ \"$3\" = '{{range .Config.Env}}{{println .}}{{end}}' ]; then printf 'USERNAME=fixture\\nPASSWORD=fixture\\n'; elif [ \"$2\" = --format ]; then python3",
        );
        fs::write(docker, script).unwrap();
        fs::write(self.path("probe-outcome"), outcome).unwrap();
        fs::write(self.path("bin/xfreerdp3"), r#"#!/usr/bin/env python3
import base64, datetime, hashlib, hmac, json, os, pathlib, re, sys, time
root = pathlib.Path(os.environ['MENDIMARU_STOP_FIXTURE_ROOT'])
args = sys.stdin.read().splitlines()
(root / 'probe-entered').touch()
bootstrap = pathlib.Path(next(a.split(',', 1)[1] for a in args if a.startswith('/drive:MxAuth'))) / 'bootstrap.ps1'
launcher = bootstrap.read_text()
def field(name, text=launcher):
    return re.search(r'\$' + name + r"\s*=\s*'([^']*)'", text).group(1)
source = root / '.mendimaru/commands' / field('source').split('\\')[-1]
script = source.read_text()
assert field('mode', script) in ['probe', 'query']
if field('mode', script) == 'probe':
    assert '$targetProcessId = [int]4242' in script
    assert '$targetStartedTicks = [long]638908128000000000' in script
report = root / '.mendimaru/operations' / field('resultPath', script).split('\\')[-1]
outcome = (root / 'probe-outcome').read_text()
if outcome == 'certificate-rejected':
    print('certificate not trusted', file=sys.stderr, flush=True)
    time.sleep(15)
    sys.exit(1)
sessions = []
if outcome in ['live', 'wrong-identity']:
    pid = 4242 if outcome == 'live' else 4343
    sessions = [dict(sessionId=f'studio-{pid}-638908128000000000', version='11.12.3', processId=pid, startedAt='2025-08-15T00:00:00Z', hasWindow=True)]
payload = dict(state='failed' if outcome == 'failed' else 'succeeded', timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(), sessions=sessions)
encoded = base64.b64encode(json.dumps(payload).encode()).decode()
request, nonce, key = field('requestId'), field('nonce'), base64.b64decode(field('key'))
mac = hmac.new(key, f'{request}\n{nonce}\n1\n{encoded}'.encode(), hashlib.sha256).hexdigest()
if outcome == 'tampered': mac = '0' * 64
host = next(a[3:] for a in args if a.startswith('/v:'))
port = next(a[6:] for a in args if a.startswith('/port:'))
pin = pathlib.Path(os.environ['XDG_CONFIG_HOME']) / 'freerdp/server' / f'{host}_{port}.pem'
pin.write_text('-----BEGIN CERTIFICATE-----\nbWVuZGltYXJ1\n-----END CERTIFICATE-----\n')
pin.chmod(0o600)
report.write_text(json.dumps(dict(schemaVersion=1, requestId=request, nonce=nonce, sequence=1, payload=encoded, mac=mac)))
time.sleep(15)
"#).unwrap();
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
    if mode == "launch-prepare" || mode.starts_with("studio-status") || mode == "studio-stop" {
        let paths = AppPaths::discover_for_cli().unwrap();
        let config = crate::application::load_config(&paths).unwrap();
        crate::winboat::seed_installed_versions_cache(
            &config,
            &[crate::models::StudioVersion {
                version: "11.12.3".into(),
                display_name: "Studio Pro 11.12.3".into(),
                executable_path: r"C:\Program Files\Mendix\11.12.3\modeler\studiopro.exe".into(),
                install_root: r"C:\Program Files\Mendix\11.12.3".into(),
                source: "fixture".into(),
                removable: true,
            }],
        );
        if mode.starts_with("studio-status") || mode == "studio-stop" {
            let mut args = vec![
                "studio",
                if mode == "studio-stop" {
                    "stop"
                } else {
                    "status"
                },
                "--json",
                "--timeout-seconds",
                "90",
            ];
            if mode != "studio-stop" && !mode.ends_with("cached") {
                args.push("--refresh");
            }
            if mode == "studio-stop" || mode.contains("exact") {
                args.extend(["--session-id", STUDIO_ID]);
            }
            if mode.contains("orphans") {
                args.push("--orphans");
            }
            let execution =
                execute(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).unwrap();
            fs::write(
                root.join(format!("{mode}.json")),
                if execution.exit_code == EXIT_OK {
                    execution.stdout
                } else {
                    execution.stderr
                },
            )
            .unwrap();
            return;
        }
        let result = tauri::async_runtime::block_on(
            crate::winboat::runtime::prepare_studio_session(&config, None, 3600),
        );
        let document = match result {
            Ok(id) => json!({"ok": true, "runtimeSessionId": id}),
            Err(error) => json!({"ok": false, "error": sanitize_backend_error(error)}),
        };
        fs::write(
            root.join("launch-prepare.json"),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
    } else if mode == "lock-owner" {
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
    } else if mode == "current-lock-owner" {
        // A current-version stop holds the private coordination lock (#225).
        let config: crate::models::AppConfig =
            serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
        let _lease = tauri::async_runtime::block_on(crate::winboat::maintenance::runtime_stop(
            &config,
            Duration::ZERO,
        ))
        .unwrap();
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
    } else if mode.starts_with("legacy-keeper") {
        use std::os::unix::net::UnixListener;
        let paths = AppPaths::discover_for_cli().unwrap();
        let directory = ensure_session_socket_directory(&paths).unwrap();
        let socket = directory.join(session_socket_name(STUDIO_ID));
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(root.join("keeper-ready"), b"ready").unwrap();
        loop {
            let (mut peer, _) = listener.accept().unwrap();
            let mut line = String::new();
            use std::io::BufRead;
            std::io::BufReader::new(peer.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let response = if line.trim() == "status" {
                json!({"ok":true,"session":{
                    "schemaVersion":CONTRACT_SCHEMA_VERSION,
                    "sessionId":if mode.ends_with("wrong-session") { "studio-9999-638908128000000000" } else { STUDIO_ID },
                    "version":"11.12.3","state":"unknown","processId":4242,
                    "connection":"disconnected","reconnectable":false
                }})
            } else {
                json!({"ok":false,"session":null})
            };
            writeln!(peer, "{response}").unwrap();
        }
    } else if mode == "keeper" || mode == "keeper-observe" || mode == "keeper-readiness-failed" {
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
        if root.join("probe-outcome").exists() {
            let query_root = root.clone();
            thread::spawn(move || {
                use crate::winboat::security::{
                    authenticate_report, authenticated_envelope, OperationSecurity,
                };
                let request = query_root.join("client-control.json.sessions.request");
                let report = query_root.join("client-control.json.sessions.report");
                let security = OperationSecurity::fixture();
                let mut sequence = 10;
                loop {
                    if let Ok(bytes) = fs::read(&request) {
                        if let Ok(auth) = authenticate_report(&bytes, &security) {
                            let packet: Value = serde_json::from_slice(&auth.payload).unwrap();
                            let outcome =
                                fs::read_to_string(query_root.join("probe-outcome")).unwrap();
                            let mut sessions = Vec::new();
                            if outcome == "live" || outcome == "wrong-identity" {
                                let pid = if outcome == "live" { 4242 } else { 4343 };
                                sessions.push(json!({"sessionId":format!("studio-{pid}-638908128000000000"),
                                    "version":"11.12.3","processId":pid,"startedAt":"2025-08-15T00:00:00Z","hasWindow":true}));
                            }
                            let value = json!({"id":if outcome == "replayed" { json!("query_old") } else {packet["id"].clone()},
                                "sessionId":STUDIO_ID,"ok":outcome != "failed","sessions":sessions});
                            sequence += 1;
                            let mut envelope = authenticated_envelope(
                                &security,
                                sequence,
                                &serde_json::to_vec(&value).unwrap(),
                            )
                            .unwrap();
                            if outcome == "tampered" {
                                let last = envelope.len() - 2;
                                envelope[last] ^= 1;
                            }
                            let _ = fs::remove_file(&request);
                            fs::write(query_root.join("query-response.tmp"), envelope).unwrap();
                            fs::rename(query_root.join("query-response.tmp"), &report).unwrap();
                            fs::write(query_root.join("monitor-query-entered"), b"queried")
                                .unwrap();
                        }
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            });
        }
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
        if mode == "keeper-readiness-failed" {
            assert_eq!(
                finish_session_keeper_launch(SessionKeeperResponse {
                    ok: false,
                    operation_id: None,
                    studio_session_id: Some(STUDIO_ID.into()),
                    error: Some(BackendError::operation(
                        BackendId::LinuxWinboat,
                        CapabilityId::StudioStart,
                        crate::winboat::STUDIO_UI_UNAVAILABLE,
                    )),
                }),
                EXIT_OK
            );
        } else {
            let (listener, guard, session_id) = prepare_session_keeper(STUDIO_ID).unwrap();
            fs::write(root.join("keeper-ready"), b"ready").unwrap();
            tauri::async_runtime::block_on(serve_session_keeper(listener, guard, &session_id));
        }
        assert!(crate::winboat::registered_client_sessions().is_empty());
        fs::write(root.join("keeper-finished"), b"finished").unwrap();
    } else if mode == "runtime-start" {
        let execution = execute(
            &[
                "runtime",
                "start",
                "--mode",
                "studio-run-locally",
                "--json",
                "--timeout-seconds",
                "15",
            ]
            .map(OsString::from),
        )
        .unwrap();
        fs::write(root.join("runtime-start.json"), execution.stderr).unwrap();
    } else if mode == "vm-recreate" {
        let config =
            crate::application::load_config(&AppPaths::discover_for_cli().unwrap()).unwrap();
        let result = tauri::async_runtime::block_on(crate::winboat::recreate_container(&config));
        assert_eq!(result.unwrap_err(), crate::winboat::vm_use::BUSY);
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
    } else if mode == "frontend-health" {
        let execution = execute(
            &[
                "browser",
                "frontend-health",
                "--runtime-session-id",
                RUNTIME_ID,
                "--observation-ms",
                "100",
                "--json",
                "--timeout-seconds",
                "15",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert!(execution.stderr.is_empty(), "{}", execution.stderr);
        fs::write(root.join("frontend-health.json"), execution.stdout).unwrap();
    } else if mode == "browser-url" || mode == "frontend-url" {
        // Join from another config/cache/Compose copy with no Runtime record.
        let mut config =
            crate::application::load_config(&AppPaths::discover_for_cli().unwrap()).unwrap();
        let isolated = root.join("isolated");
        fs::create_dir_all(&isolated).unwrap();
        fs::copy(&config.compose_file, isolated.join("compose.yml")).unwrap();
        config.compose_file = isolated.join("compose.yml").to_string_lossy().into_owned();
        fs::write(
            isolated.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        std::env::set_var("MENDIMARU_CONFIG_DIR", &isolated);
        std::env::set_var("MENDIMARU_CACHE_DIR", isolated.join("cache"));
        let suite = root.join("smoke.browser.json");
        let frontend = mode == "frontend-url";
        let url = format!("{}{}", config.api_url, if frontend { "/hold" } else { "" });
        let mut args = vec![
            "browser",
            if frontend { "frontend-health" } else { "test" },
            "--base-url",
            &url,
            "--winboat-use",
            "--json",
            "--timeout-seconds",
            "15",
        ];
        if frontend {
            args.extend(["--observation-ms", "100"]);
        } else {
            args.extend(["--suite-path", suite.to_str().unwrap()]);
        }
        let execution = execute(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).unwrap();
        fs::write(
            root.join(format!("{mode}.json")),
            if execution.exit_code == EXIT_OK {
                execution.stdout
            } else {
                execution.stderr
            },
        )
        .unwrap();
    } else if mode == "browser" || mode == "browser-local" || mode == "browser-build" {
        if mode == "browser-local" {
            let config =
                crate::application::load_config(&AppPaths::discover_for_cli().unwrap()).unwrap();
            let client = Command::new("sleep").arg("60").spawn().unwrap();
            crate::winboat::register_keeper_test_client(&config, client);
        }
        let suite = root.join("smoke.browser.json");
        let marker = root.join("build-generation");
        let mut args = vec![
            "browser",
            "test",
            "--runtime-session-id",
            RUNTIME_ID,
            "--suite-path",
            suite.to_str().unwrap(),
            "--json",
            "--timeout-seconds",
            "30",
        ];
        if mode == "browser-build" {
            args.extend(["--build-marker", marker.to_str().unwrap()]);
        }
        let execution = execute(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).unwrap();
        let output = if !execution.stdout.is_empty() {
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
        let output = if !execution.stdout.is_empty() {
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
        fixture.spawn("frontend-health").finish();
        let health = fixture.result("frontend-health");
        assert_eq!(health["data"]["frontendState"], "healthy", "{health}");
        assert_eq!(health["data"]["httpReady"], true);
        assert_eq!(health["data"]["studioState"], "running");
        assert_eq!(health["data"]["assetBypass"], false);
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
    // "lock-owner" is an older version holding the legacy Compose-directory
    // lock in a directory it trusts; "current-lock-owner" holds the private lock.
    for owner_mode in ["lock-owner", "current-lock-owner"] {
        let fixture = Fixture::new();
        // The legacy owner models an older version, which only coordinates in
        // a Compose directory that is not group/other writable.
        fs::set_permissions(fixture.root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let owner = fixture.spawn(owner_mode);
        until(|| fixture.path("lock-owned").exists());
        let mut stop = fixture.spawn("stop");
        until(|| fixture.path("stop-started").exists());
        thread::sleep(Duration::from_millis(100));
        assert_eq!(fixture.calls(), 0, "{owner_mode}");
        drop(owner);
        stop.finish();
        assert_eq!(
            fixture.result("stop")["data"]["completed"],
            true,
            "{owner_mode}"
        );
        fixture.assert_stopped(1);
    }
}

#[test]
fn runtime_stop_fails_closed_on_an_existing_untrusted_legacy_entry() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let target = fixture.path("lock-target");
    fs::write(&target, b"untouched").unwrap();
    symlink(&target, fixture.path(".mendimaru-runtime-stop.lock")).unwrap();
    fs::set_permissions(fixture.root.path(), fs::Permissions::from_mode(0o775)).unwrap();
    fixture.spawn("stop").finish();
    fs::set_permissions(fixture.root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        fixture.result("stop")["error"]["code"],
        "coordination_untrusted"
    );
    assert_eq!(fixture.calls(), 0);
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
}

#[test]
fn runtime_stop_refuses_untrusted_lock_files_without_touching_compose() {
    use std::os::unix::fs::symlink;
    for kind in ["symlink", "hardlink", "public", "directory"] {
        let fixture = Fixture::new();
        let config: crate::models::AppConfig =
            serde_json::from_slice(&fs::read(fixture.path("config.json")).unwrap()).unwrap();
        // The private coordination lock is the one every version must trust.
        let lock = crate::winboat::maintenance::private_lock_path(&config, true);
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
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
        assert_eq!(result["error"]["code"], "coordination_untrusted", "{kind}");
        assert_eq!(result["error"]["capability"], "runtime.stop", "{kind}");
        assert_eq!(
            result["error"]["message"],
            crate::winboat::maintenance::UNTRUSTED,
            "{kind}"
        );
        assert_eq!(result["error"]["retryable"], false, "{kind}");
        assert_eq!(fixture.calls(), 0);
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        if kind == "directory" {
            fs::remove_dir(&lock).unwrap();
        } else {
            fs::remove_file(&lock).unwrap();
        }
        assert!(!result
            .to_string()
            .contains(fixture.root.path().to_str().unwrap()));
    }
}

#[test]
fn live_browser_use_prevents_stop_start_and_recreate_until_test_finishes() {
    assert_browser_protects_vm("browser");
}

#[test]
fn plain_url_browser_in_another_cache_protects_the_same_vm() {
    assert_browser_protects_vm("browser-url");
}

#[test]
fn frontend_health_protects_linked_runtime_and_explicit_vm_url() {
    assert_browser_protects_vm("frontend-health");
    assert_browser_protects_vm("frontend-url");
}

fn assert_browser_protects_vm(browser_mode: &str) {
    let fixture = Fixture::new();
    if browser_mode == "frontend-health" {
        fs::write(fixture.path("frontend-hold"), b"").unwrap();
    }
    fixture.prepare_browser();
    let suite = fixture.path("smoke.browser.json");
    fs::write(
        &suite,
        fs::read_to_string(&suite)
            .unwrap()
            .replace("\"path\":\"/\"", "\"path\":\"/hold\""),
    )
    .unwrap();
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    let compose = fs::read(fixture.path("compose.yml")).unwrap();
    let inspection = fs::read(fixture.path("inspect.json")).unwrap();
    let mut browser = fixture.spawn(browser_mode);
    until(|| fixture.path("browser-held").exists());
    // Request actual CLI lifecycle operations while Chromium is inside the test.
    let mut stop = fixture.spawn("stop");
    let mut start = fixture.spawn("runtime-start");
    let mut recreate = fixture.spawn("vm-recreate");
    stop.finish();
    start.finish();
    recreate.finish();
    for mode in ["stop", "runtime-start"] {
        let result = fixture.result(mode);
        assert_eq!(result["error"]["code"], "precondition_failed", "{result}");
        assert_eq!(result["error"]["message"], crate::winboat::vm_use::BUSY);
        assert_eq!(result["error"]["retryable"], true);
        assert!(!result
            .to_string()
            .contains(fixture.root.path().to_str().unwrap()));
    }
    assert_eq!(fixture.calls(), 0);
    assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
    assert_eq!(fs::read(fixture.path("inspect.json")).unwrap(), inspection);
    assert!(keeper.0.try_wait().unwrap().is_none());
    fs::write(fixture.path("browser-release"), b"").unwrap();
    browser.finish();
    if browser_mode.starts_with("frontend-") {
        assert_eq!(
            fixture.result(browser_mode)["data"]["frontendState"],
            "healthy"
        );
    } else {
        assert_eq!(fixture.result(browser_mode)["data"]["outcome"], "passed");
    }
    fixture.confirm_studio_exit();
    keeper.finish();
    fixture.assert_stopped(1);
}

// Intentional external-control simulation is a separate gate, never an ordinary
// lifecycle test or a claim about Docker/Windows itself.
#[test]
#[ignore = "external change fixture gate; run explicitly with --ignored"]
fn external_environment_changes_preserve_browser_evidence() {
    for component in ["container", "published-ports", "build", "runtime"] {
        let fixture = Fixture::new();
        fixture.prepare_browser();
        fs::write(fixture.path("build-generation"), b"build-one").unwrap();
        let suite = fixture.path("smoke.browser.json");
        fs::write(
            &suite,
            fs::read_to_string(&suite)
                .unwrap()
                .replace("\"path\":\"/\"", "\"path\":\"/hold\""),
        )
        .unwrap();
        let mut keeper = fixture.spawn("keeper-observe");
        until(|| fixture.path("keeper-ready").exists());
        let compose = fs::read(fixture.path("compose.yml")).unwrap();
        let mut browser = fixture.spawn("browser-build");
        until(|| fixture.path("browser-held").exists());
        if component == "build" {
            fs::write(fixture.path("next-build"), b"build-one").unwrap();
            fs::rename(fixture.path("next-build"), fixture.path("build-generation")).unwrap();
        } else if component == "runtime" {
            let file = fixture.path(&format!(
                "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
            ));
            let mut record: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            record["startedAt"] = json!("2026-09-09T00:00:00Z");
            fs::write(file, serde_json::to_vec(&record).unwrap()).unwrap();
        } else {
            let file = fixture.path("inspect.json");
            let mut inspection: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            if component == "container" {
                inspection[0]["Id"] = json!("b".repeat(64));
            } else {
                inspection[0]["NetworkSettings"]["Ports"]["3389/tcp"] =
                    json!([{"HostIp":"127.0.0.1","HostPort":"12345"}]);
            }
            fs::write(file, serde_json::to_vec(&inspection).unwrap()).unwrap();
        }
        // Leave navigation pending long enough for the periodic observer to
        // interrupt it, then unblock the fixture server for orderly shutdown.
        thread::sleep(Duration::from_secs(3));
        fs::write(fixture.path("browser-release"), b"").unwrap();
        browser.finish();
        let result = fixture.result("browser-build");
        let data = &result["data"];
        assert_eq!(data["outcome"], "failed", "{result}");
        assert_eq!(data["environment"]["comparable"], false);
        assert_eq!(data["environment"]["missing"], json!([]));
        assert_eq!(data["environment"]["actor"], "unknown");
        assert!(
            data["environment"]["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["component"] == component),
            "{result}"
        );
        assert!(!data["environment"]
            .to_string()
            .contains(fixture.root.path().to_str().unwrap()));
        assert!(data["tests"][0]["failure"]
            .as_str()
            .unwrap()
            .contains("environment observation"));
        assert_eq!(fixture.calls(), 0);
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        assert!(!fixture.path("unexpected-rdp").exists());
        assert!(keeper.0.try_wait().unwrap().is_none());
        fixture.confirm_studio_exit();
        keeper.finish();
        fixture.assert_stopped(1);
    }
}

#[test]
fn studio_launch_after_owner_loss_reuses_only_an_authoritatively_ended_identity() {
    for outcome in [
        "absent",
        "live",
        "failed",
        "tampered",
        "wrong-identity",
        "certificate-rejected",
    ] {
        let fixture = Fixture::new();
        fixture.prepare_launch_recovery(outcome);
        let directory = fixture.path("cache/cli-sessions");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("socket-cleanup.log"),
            b"retained cleanup audit\n",
        )
        .unwrap();
        let compose = fs::read(fixture.path("compose.yml")).unwrap();
        let record_path = fixture.path(&format!(
            "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
        ));
        let previous = fs::read(&record_path).unwrap();
        fixture.spawn("launch-prepare").finish();
        let result = fixture.result("launch-prepare");
        assert!(
            fixture.path("probe-entered").exists(),
            "{outcome}: probe was not attempted: {result}"
        );
        assert_eq!(fixture.calls(), 0, "orphan inspection recreated Compose");
        assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
        if outcome == "absent" {
            assert_eq!(result["ok"], true, "{result}");
            assert_eq!(result["runtimeSessionId"], RUNTIME_ID);
            let record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
            assert!(record["studioSessionId"].is_null());
            assert!(
                fs::read_to_string(record_path.parent().unwrap().join("runtime.log"))
                    .unwrap()
                    .contains("Stale Studio link cleared")
            );
            // The next application process reuses the recovered record without
            // performing another guest probe or allocating a new Runtime.
            fs::remove_file(fixture.path("probe-entered")).unwrap();
            fixture.spawn("launch-prepare").finish();
            assert_eq!(
                fixture.result("launch-prepare")["runtimeSessionId"],
                RUNTIME_ID
            );
            assert!(!fixture.path("probe-entered").exists());
        } else {
            assert_eq!(result["ok"], false, "{result}");
            assert_eq!(fs::read(&record_path).unwrap(), previous);
            if outcome == "live" {
                assert_eq!(result["error"]["code"], "runtime_port_conflict");
            } else if outcome == "certificate-rejected" {
                assert_eq!(
                    result["error"]["code"],
                    "remote_desktop_certificate_mismatch"
                );
                assert_eq!(result["error"]["retryable"], false);
            } else {
                assert_eq!(result["error"]["code"], "precondition_failed");
                assert_eq!(result["error"]["retryable"], true);
                assert_eq!(
                    result["error"]["message"],
                    crate::winboat::runtime::STUDIO_SESSION_UNVERIFIED
                );
            }
        }
    }
}

#[test]
fn studio_launch_recovers_reboot_runtime_with_cleanup_audit_and_dead_keeper() {
    use std::os::unix::net::UnixListener;

    for stale_socket in [false, true] {
        let fixture = Fixture::new();
        fixture.prepare_launch_recovery("absent");
        let directory = fixture.path("cache/cli-sessions");
        fs::create_dir_all(&directory).unwrap();
        let audit = directory.join("socket-cleanup.log");
        fs::write(&audit, b"retained cleanup audit\n").unwrap();
        let socket = directory.join(session_socket_name(STUDIO_ID));
        if stale_socket {
            drop(UnixListener::bind(&socket).unwrap());
            fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let record_path = fixture.path(&format!(
            "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
        ));
        let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        record["state"] = json!("starting");
        record["studioProcessId"] = Value::Null;
        record["httpReady"] = json!(false);
        fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();

        // Refresh remains safe with the same retained audit and dead socket.
        fixture.spawn("studio-status").finish();
        assert_eq!(fixture.result("studio-status")["data"], json!([]));
        fs::remove_file(fixture.path("probe-entered")).unwrap();
        fixture.spawn("launch-prepare").finish();
        let result = fixture.result("launch-prepare");
        assert_eq!(result["ok"], true, "stale_socket={stale_socket}: {result}");
        assert_eq!(result["runtimeSessionId"], RUNTIME_ID);
        assert!(fixture.path("probe-entered").exists());
        let recovered: Value = serde_json::from_slice(&fs::read(record_path).unwrap()).unwrap();
        assert!(recovered["studioSessionId"].is_null());
        assert_eq!(fs::read(&audit).unwrap(), b"retained cleanup audit\n");
        assert_eq!(socket.exists(), stale_socket);
        assert_eq!(fixture.calls(), 0, "recovery must not recreate Compose");
    }
}

#[test]
fn studio_launch_preserves_a_retained_owner_without_opening_another_rdp_connection() {
    let fixture = Fixture::new();
    fixture.prepare_launch_recovery("absent");
    let mut keeper = fixture.spawn("keeper-observe");
    until(|| fixture.path("keeper-ready").exists());
    fs::write(
        fixture.path("cache/cli-sessions/socket-cleanup.log"),
        b"audit\n",
    )
    .unwrap();
    let record = fixture.path(&format!(
        "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
    ));
    let before = fs::read(&record).unwrap();
    fixture.spawn("launch-prepare").finish();
    assert_eq!(
        fixture.result("launch-prepare")["error"]["code"],
        "runtime_port_conflict"
    );
    // A retained disconnected owner with an unreadable report remains unknown.
    fs::remove_file(fixture.path("client-report.json")).unwrap();
    fixture.spawn("launch-prepare").finish();
    assert_eq!(
        fixture.result("launch-prepare")["error"]["code"],
        "precondition_failed"
    );
    assert!(!fixture.path("probe-entered").exists());
    assert_eq!(fs::read(record).unwrap(), before);
    assert_eq!(fixture.calls(), 0);
    fixture.confirm_studio_exit();
    keeper.finish();
}

#[test]
fn studio_orphan_probe_refuses_other_or_unverifiable_keeper_owners() {
    use std::os::unix::net::UnixListener;
    for kind in [
        "responding",
        "audit-socket",
        "untrusted",
        "unexpected-file",
        "incomplete",
        "audit-symlink",
        "audit-directory",
    ] {
        let fixture = Fixture::new();
        fixture.prepare_launch_recovery("absent");
        let directory = fixture.path("cache/cli-sessions");
        fs::create_dir_all(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let audit = directory.join("socket-cleanup.log");
        match kind {
            "audit-symlink" => {
                std::os::unix::fs::symlink(fixture.path("config.json"), &audit).unwrap();
            }
            "audit-directory" => fs::create_dir(&audit).unwrap(),
            "audit-socket" => {}
            _ => fs::write(&audit, b"retained audit\n").unwrap(),
        }
        let other_socket = directory.join(session_socket_name("studio-4343-638908128000000000"));
        let server = if kind == "responding" || kind == "audit-socket" {
            let listener = UnixListener::bind(if kind == "audit-socket" {
                &audit
            } else {
                &other_socket
            })
            .unwrap();
            Some(thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 32];
                assert!(stream.read(&mut request).unwrap() > 0);
                // A responding owner with unavailable metadata must still block RDP.
                stream
                    .write_all(b"{\"ok\":false,\"session\":null}\n")
                    .unwrap();
            }))
        } else {
            if kind == "untrusted" {
                fs::write(other_socket, b"preserve untrusted entry").unwrap();
            } else if kind == "unexpected-file" {
                fs::write(directory.join("unexpected.txt"), b"unrecognized entry").unwrap();
            } else if kind == "incomplete" {
                for index in 0..65 {
                    let socket = directory.join(format!("s-{index:032x}.sock"));
                    drop(UnixListener::bind(socket).unwrap());
                }
            }
            None
        };
        let record = fixture.path(&format!(
            "cache/winboat-runtime/sessions/{RUNTIME_ID}/session.json"
        ));
        let before = fs::read(&record).unwrap();
        fixture.spawn("launch-prepare").finish();
        if let Some(server) = server {
            server.join().unwrap();
        }
        let result = fixture.result("launch-prepare");
        assert_eq!(
            result["error"]["code"], "precondition_failed",
            "{kind}: {result}"
        );
        assert_eq!(result["error"]["retryable"], true);
        assert!(!fixture.path("probe-entered").exists());
        assert_eq!(fs::read(record).unwrap(), before);
        assert_eq!(fixture.calls(), 0);
    }
}

#[test]
fn studio_refresh_queries_guest_with_keeper_and_preserves_ownership() {
    for outcome in [
        "absent",
        "live",
        "failed",
        "wrong-identity",
        "tampered",
        "replayed",
    ] {
        let fixture = Fixture::new();
        fixture.prepare_launch_recovery(outcome);
        let mut keeper = fixture.spawn("keeper-observe");
        until(|| fixture.path("keeper-ready").exists());
        fixture.spawn("studio-status").finish();
        assert!(fixture.path("monitor-query-entered").exists());
        assert!(!fixture.path("probe-entered").exists());
        // Disconnect the retained transport and remove the observation report:
        // the keeper must return unknown until an explicit guest refresh.
        fs::write(fixture.path("client-disconnected"), b"disconnected").unwrap();
        fs::remove_file(fixture.path("client-report.json")).unwrap();
        fixture.spawn("studio-status-cached").finish();
        assert!(!fixture.path("probe-entered").exists());
        assert_eq!(
            fixture.result("studio-status-cached")["data"][0]["state"],
            "unknown"
        );
        let compose = fs::read(fixture.path("compose.yml")).unwrap();
        for mode in [
            "studio-status",
            "studio-status-exact",
            "studio-status-orphans",
        ] {
            fixture.spawn(mode).finish();
            let result = fixture.result(mode);
            assert!(
                fixture.path("monitor-query-entered").exists(),
                "{outcome}: {result}"
            );
            assert!(
                !fixture.path("probe-entered").exists(),
                "refresh must not open RDP"
            );
            if matches!(outcome, "failed" | "tampered" | "replayed") {
                assert_eq!(result["ok"], false, "{result}");
                assert_eq!(result["error"]["capability"], "studio.status");
            } else {
                assert_eq!(result["ok"], true, "{outcome}: {result}");
                if mode.ends_with("orphans") {
                    assert_eq!(
                        result["data"].as_array().unwrap().len(),
                        usize::from(outcome == "wrong-identity")
                    );
                } else {
                    let status = if mode.ends_with("exact") {
                        &result["data"]
                    } else {
                        result["data"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|s| s["sessionId"] == STUDIO_ID)
                            .unwrap()
                    };
                    assert_eq!(
                        status["state"],
                        if outcome == "live" {
                            "running"
                        } else {
                            "stopped"
                        }
                    );
                    assert_eq!(status["connection"], "disconnected");
                    assert_eq!(status["reconnectable"], outcome == "live");
                }
            }
            assert!(keeper.0.try_wait().unwrap().is_none());
            assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
            assert_eq!(fixture.calls(), 0);
        }
        fixture.confirm_studio_exit();
        keeper.finish();
    }
}

#[test]
fn public_stop_retires_a_disconnected_keeper_only_after_authoritative_absence() {
    for outcome in ["absent", "live", "failed"] {
        let fixture = Fixture::new();
        fixture.prepare_launch_recovery(outcome);
        let mut keeper = fixture.spawn("keeper-observe");
        until(|| fixture.path("keeper-ready").exists());
        fs::write(fixture.path("client-disconnected"), b"disconnected").unwrap();
        fs::remove_file(fixture.path("client-report.json")).unwrap();
        until(|| {
            fixture.studio_status().connection
                == crate::contracts::StudioConnectionState::Disconnected
        });
        fixture
            .spawn("studio-stop")
            .finish_with_timeout(Duration::from_secs(90));
        let result = fixture.result("studio-stop");
        assert_eq!(result["ok"], outcome == "absent", "{outcome}: {result}");
        if outcome == "absent" {
            keeper.finish();
            assert!(!fixture
                .path("cache/cli-sessions")
                .join(session_socket_name(STUDIO_ID))
                .exists());
            fixture.assert_stopped(1);
        } else {
            assert!(keeper.0.try_wait().unwrap().is_none());
            assert_eq!(fixture.calls(), 0);
            fixture.confirm_studio_exit();
            keeper.finish();
        }
    }
}

#[test]
fn stopped_legacy_keeper_retirement_pins_the_verified_ipc_peer() {
    use std::os::unix::process::ExitStatusExt;
    for mode in ["legacy-keeper", "legacy-keeper-wrong-session"] {
        let fixture = Fixture::new();
        fixture.prepare_launch_recovery("absent");
        let mut keeper = fixture.spawn(mode);
        until(|| fixture.path("keeper-ready").exists());
        fixture
            .spawn("studio-stop")
            .finish_with_timeout(Duration::from_secs(90));
        let result = fixture.result("studio-stop");
        if mode == "legacy-keeper" {
            assert_eq!(result["ok"], true, "{result}");
            until(|| keeper.0.try_wait().unwrap().is_some());
            assert_eq!(keeper.0.wait().unwrap().signal(), Some(libc::SIGTERM));
            assert!(!fixture
                .path("cache/cli-sessions")
                .join(session_socket_name(STUDIO_ID))
                .exists());
            fixture.assert_stopped(1);
        } else {
            assert_eq!(result["ok"], false, "{result}");
            assert!(keeper.0.try_wait().unwrap().is_none());
            assert_eq!(fixture.calls(), 0);
        }
    }
}

#[test]
fn failed_ui_readiness_keeps_acknowledged_owner_and_runtime_until_confirmed_exit() {
    let fixture = Fixture::new();
    let compose = fs::read(fixture.path("compose.yml")).unwrap();
    let mut keeper = fixture.spawn("keeper-readiness-failed");
    keeper
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(b"accept\n")
        .unwrap();
    until(|| {
        fixture
            .path(&format!(
                "cache/cli-sessions/{}",
                session_socket_name(STUDIO_ID)
            ))
            .exists()
    });
    assert!(keeper.0.try_wait().unwrap().is_none());
    assert_eq!(fixture.studio_status().session_id, STUDIO_ID);
    assert_eq!(fs::read(fixture.path("compose.yml")).unwrap(), compose);
    assert_eq!(fixture.calls(), 0);
    fixture.confirm_studio_exit();
    keeper.finish();
    fixture.assert_stopped(1);
}
