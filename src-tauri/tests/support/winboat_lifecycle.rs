use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

pub(crate) const CURRENT_SCHEMA: &str = "4.0.0";
pub(crate) const LEGACY_SCHEMA: &str = "3.0.0";
pub(crate) const LEGACY_SESSION_ID: &str = "runtime_2dc6d67e680e1fda66e95b01f2891075";

pub(crate) struct WinBoatLifecycleFixture {
    _root: TempDir,
    pub(crate) config_directory: PathBuf,
    pub(crate) cache_directory: PathBuf,
    pub(crate) workspace: PathBuf,
}

impl WinBoatLifecycleFixture {
    pub(crate) fn new() -> Self {
        let root = TempDir::new().expect("temporary lifecycle root");
        let config_directory = root.path().join("config");
        let cache_directory = root.path().join("cache");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&config_directory).expect("configuration directory");
        fs::create_dir_all(&cache_directory).expect("cache directory");
        fs::create_dir_all(&workspace).expect("workspace");
        let config = json!({
            "languagePreference": "en-US",
            "winboatSetupPending": false,
            "winboatExecutable": "fixture-winboat",
            "composeFile": "missing-compose.yml",
            "containerRuntime": "docker",
            "containerName": "WinBoat",
            "apiUrl": "http://127.0.0.1:9",
            "rdpHost": "127.0.0.1",
            "rdpPort": 9,
            "sharedDirectory": workspace.to_string_lossy(),
            "windowsSharedDirectory": "\\\\fixture\\Data",
            "freerdpBinary": "fixture-freerdp",
            "mendixInstallRoot": "C:\\Program Files\\Mendix",
            "mendixDataRoot": "C:\\ProgramData\\Mendix",
            "windowsStudioPaths": [],
            "startupTimeoutSeconds": 1
        });
        fs::write(
            config_directory.join("config.json"),
            serde_json::to_vec_pretty(&config).expect("configuration JSON"),
        )
        .expect("configuration fixture");
        Self {
            _root: root,
            config_directory,
            cache_directory,
            workspace,
        }
    }

    pub(crate) fn write_current_runtime_record(
        &self,
        suffix: char,
        state: &str,
        studio_session_id: Option<&str>,
    ) -> String {
        let session_id = format!("runtime_{}", suffix.to_string().repeat(32));
        let record = json!({
            "schemaVersion": CURRENT_SCHEMA,
            "sessionId": session_id,
            "backend": "linux-winboat",
            "mode": "studio-run-locally",
            "studioSessionId": studio_session_id,
            "studioState": if studio_session_id.is_some() { "running" } else { "unknown" },
            "studioProcessId": null,
            "state": state,
            "httpReady": false,
            "hostPort": 8_080,
            "guestPort": 8_080,
            "startedAt": "2026-09-03T00:00:00Z",
            "readinessTimeoutSeconds": 3_600,
            "failureCode": null,
            "logArtifact": {
                "schemaVersion": CURRENT_SCHEMA,
                "artifactId": format!("artifact_{}", format!("{suffix}").repeat(32)),
                "sessionId": session_id,
                "backend": "linux-winboat",
                "kind": "runtime-log",
                "createdAt": "2026-09-03T00:00:00Z"
            },
            "composeChanged": true,
            "originalComposeSha256": "1".repeat(64),
            "managedComposeSha256": "2".repeat(64),
            "storageMountIdentity": ["fixture-storage"]
        });
        self.write_runtime_record(&session_id, &record);
        session_id
    }

    pub(crate) fn write_legacy_runtime_record(&self) -> String {
        let record = json!({
            "schemaVersion": LEGACY_SCHEMA,
            "sessionId": LEGACY_SESSION_ID,
            "mode": "studio-run-locally",
            "studioSessionId": null,
            "state": "starting",
            "hostPort": 32_768,
            "guestPort": 8_080,
            "composeChanged": true
        });
        self.write_runtime_record(LEGACY_SESSION_ID, &record);
        LEGACY_SESSION_ID.to_string()
    }

    pub(crate) fn runtime_record_path(&self, session_id: &str) -> PathBuf {
        self.cache_directory
            .join("winboat-runtime")
            .join("sessions")
            .join(session_id)
            .join("session.json")
    }

    pub(crate) fn runtime_session_directory(&self, session_id: &str) -> PathBuf {
        self.runtime_record_path(session_id)
            .parent()
            .expect("Runtime session directory")
            .to_path_buf()
    }

    pub(crate) fn write_compose(&self, variant: ComposeVariant) {
        let runtime_mapping = match variant {
            ComposeVariant::Clean => String::new(),
            ComposeVariant::Dynamic => "      - 127.0.0.1::8080/tcp\n".to_string(),
            ComposeVariant::Public => "      - 0.0.0.0:8080:8080/tcp\n".to_string(),
            ComposeVariant::FixedStale => "      - 127.0.0.1:8080:8080/tcp\n".to_string(),
        };
        let content = format!(
            "services:\n  windows:\n    image: ghcr.io/dockur/windows:6.03\n    container_name: WinBoat\n    volumes:\n      - winboat-data:/storage\n    ports:\n      - 127.0.0.1:47280:7148/tcp\n      - 127.0.0.1:5900:5900/tcp\n{runtime_mapping}volumes:\n  winboat-data: {{}}\n"
        );
        fs::write(self.workspace.join("docker-compose.yml"), content)
            .expect("Compose lifecycle fixture");
    }

    pub(crate) fn compose_text(&self) -> String {
        fs::read_to_string(self.workspace.join("docker-compose.yml")).expect("Compose fixture")
    }

    pub(crate) fn write_dead_lock(&self, process_id: u32) -> PathBuf {
        let lock = self.workspace.join("IronCalcSpreadUIShowcase.mpr.lock");
        fs::write(
            &lock,
            format!(
                r#"{{"SessionId":"f205dd03-790d-43fa-a77d-be3b6b7ff1b7","ProcessId":{process_id}}}"#
            ),
        )
        .expect("dead project lock");
        lock
    }

    pub(crate) fn write_orphan_keeper_socket(&self, name: &str) -> PathBuf {
        let directory = self.cache_directory.join("cli-sessions");
        fs::create_dir_all(&directory).expect("session keeper directory");
        let socket = directory.join(name);
        let listener =
            std::os::unix::net::UnixListener::bind(&socket).expect("temporary keeper socket");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .expect("keeper socket permissions");
        drop(listener);
        socket
    }

    pub(crate) fn run_cli(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mendimaru"))
            .args(arguments)
            .env("MENDIMARU_CONFIG_DIR", &self.config_directory)
            .env("MENDIMARU_CACHE_DIR", &self.cache_directory)
            .output()
            .expect("run lifecycle CLI")
    }

    fn write_runtime_record(&self, session_id: &str, record: &Value) {
        let directory = self
            .cache_directory
            .join("winboat-runtime")
            .join("sessions")
            .join(session_id);
        write_private_directory(&directory);
        fs::write(
            directory.join("session.json"),
            serde_json::to_vec_pretty(record).expect("Runtime record JSON"),
        )
        .expect("Runtime lifecycle record");
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ComposeVariant {
    Clean,
    Dynamic,
    Public,
    FixedStale,
}

fn write_private_directory(path: &Path) {
    fs::create_dir_all(path).expect("fixture directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("private fixture directory");
}
