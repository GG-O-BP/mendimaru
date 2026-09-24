use super::*;
use crate::contracts::{BackendError, CapabilityId, StudioProcessState};
use crate::models::AppConfig;
use crate::process::{self, CommandPolicy};
use notify::{RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_MARKER: u64 = 64 * 1024;

pub(crate) struct Observer {
    url: String,
    initial: Report,
    worker: tokio::task::JoinHandle<()>,
}
impl Observer {
    pub(crate) fn initial(&self) -> &Report {
        &self.initial
    }
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

struct BuildWatch {
    path: PathBuf,
    generation: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    _watcher: notify::RecommendedWatcher,
}
impl BuildWatch {
    fn new(path: &Path) -> Result<Self, ()> {
        if std::fs::symlink_metadata(path)
            .map_err(|_| ())?
            .file_type()
            .is_symlink()
        {
            return Err(());
        }
        let path = path.canonicalize().map_err(|_| ())?;
        let parent = path.parent().ok_or(())?;
        let generation = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let (counter, error, target) = (generation.clone(), failed.clone(), path.clone());
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if event.need_rescan() => {
                    error.store(true, Ordering::SeqCst);
                }
                Ok(event)
                    if !matches!(event.kind, notify::EventKind::Access(_))
                        && (event
                            .paths
                            .iter()
                            .any(|p| p == &target || target.starts_with(p))
                            || event.paths.is_empty()) =>
                {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => {
                    error.store(true, Ordering::SeqCst);
                }
                _ => {}
            })
            .map_err(|_| ())?;
        // Watching the parent survives atomic marker replacement. Never recurse
        // through a project or silently recreate a watcher inside the same run.
        watcher
            .watch(parent, RecursiveMode::NonRecursive)
            .map_err(|_| ())?;
        file_digest(&path, MAX_MARKER, true).ok_or(())?;
        Ok(Self {
            path,
            generation,
            failed,
            _watcher: watcher,
        })
    }
    fn snapshot(&self) -> (Option<String>, Option<u64>) {
        if self.failed.load(Ordering::SeqCst) {
            return (None, None);
        }
        (
            file_digest(&self.path, MAX_MARKER, true),
            Some(self.generation.load(Ordering::SeqCst)),
        )
    }
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file_digest(path: &Path, limit: u64, metadata_identity: bool) -> Option<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let before = file.metadata().ok()?;
    if !before.is_file() || before.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    (&file).take(limit + 1).read_to_end(&mut bytes).ok()?;
    let after = file.metadata().ok()?;
    let identity = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if bytes.len() as u64 > limit || identity(&before) != identity(&after) {
        return None;
    }
    if metadata_identity {
        bytes.extend_from_slice(format!("{:?}", identity(&after)).as_bytes());
    }
    Some(hash(bytes))
}

#[derive(Deserialize)]
struct Inspection {
    id: String,
    running: bool,
    ports: std::collections::BTreeMap<String, Option<Vec<Binding>>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Binding {
    host_ip: String,
    host_port: String,
}

fn parse_container(bytes: &[u8]) -> Option<(String, bool, Vec<Port>)> {
    let raw: Inspection = serde_json::from_slice(bytes).ok()?;
    if raw.id.len() != 64
        || !raw
            .id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || raw.ports.len() > 256
    {
        return None;
    }
    let mut ports = Vec::new();
    for (key, bindings) in raw.ports {
        let (guest, protocol) = key.split_once('/')?;
        let guest: u16 = guest.parse().ok()?;
        if guest == 0 || !matches!(protocol, "tcp" | "udp" | "sctp") {
            return None;
        }
        for binding in bindings.unwrap_or_default() {
            let host: u16 = binding.host_port.parse().ok()?;
            let ip: std::net::IpAddr = binding.host_ip.parse().ok()?;
            if host == 0 || ports.len() >= 256 {
                return None;
            }
            ports.push(Port {
                guest,
                protocol: protocol.into(),
                host,
                address_digest: hash(ip.to_string()),
            });
        }
    }
    ports.sort_by(|a, b| {
        (a.guest, &a.protocol, a.host, &a.address_digest).cmp(&(
            b.guest,
            &b.protocol,
            b.host,
            &b.address_digest,
        ))
    });
    ports.dedup();
    Some((raw.id, raw.running, ports))
}

struct Source {
    config: AppConfig,
    management: Option<crate::winboat::vm_use::Observation>,
    runtime_id: Option<String>,
    studio_id: Option<String>,
    build: Option<BuildWatch>,
    build_requested: bool,
}
impl Source {
    async fn snapshot(&self) -> Snapshot {
        let mut command = tokio::process::Command::new(self.config.container_runtime.as_str());
        command.args(["inspect", "--format", "{\"id\":{{json .Id}},\"running\":{{json .State.Running}},\"ports\":{{json .NetworkSettings.Ports}}}", &self.config.container_name]);
        let container = async {
            let output = process::output(
                command,
                CommandPolicy::PROBE,
                None,
                "environment observation",
            )
            .await
            .ok()?;
            if !output.status.success() || output.stdout_truncated {
                return None;
            }
            parse_container(&output.stdout)
        };
        let studio = async {
            let id = self.studio_id.as_deref()?;
            let result =
                tokio::time::timeout(Duration::from_secs(2), crate::winboat::observed_session(id))
                    .await
                    .ok()?
                    .ok()??;
            if result.schema_version != crate::contracts::CONTRACT_SCHEMA_VERSION
                || result.session_id != id
            {
                return None;
            }
            Some(Studio {
                process_id: result.process_id?,
                started_at: result.started_at?,
                running: result.state == StudioProcessState::Running,
            })
        };
        let (container, studio) = tokio::join!(container, studio);
        let runtime = self
            .runtime_id
            .as_deref()
            .and_then(crate::winboat::runtime::observation)
            .map(|(id, _)| id);
        let (build, build_watch_generation) = self
            .build
            .as_ref()
            .map_or((None, None), BuildWatch::snapshot);
        let management = self.management.as_ref().and_then(|m| m.snapshot());
        Snapshot {
            observed_at: Utc::now(),
            managed_vm: management.as_ref().map(|v| v.0.clone()),
            management_generation: management.as_ref().map(|v| v.1.clone()),
            container: container.as_ref().map(|v| v.0.clone()),
            container_running: container.as_ref().map(|v| v.1),
            published_ports: container.map(|v| v.2),
            compose: file_digest(Path::new(&self.config.compose_file), MAX_FILE, false),
            runtime,
            studio,
            build,
            build_watch_generation,
        }
    }
}

pub(crate) async fn start(
    config: Option<&AppConfig>,
    runtime: Option<&str>,
    marker: Option<&str>,
) -> Result<Option<Observer>, BackendError> {
    start_prepared(config, runtime, marker, None).await
}

pub(crate) async fn start_prepared(
    config: Option<&AppConfig>,
    runtime: Option<&str>,
    marker: Option<&str>,
    preparation: Option<&Report>,
) -> Result<Option<Observer>, BackendError> {
    let config = config.filter(|_| runtime.is_none_or(crate::winboat::runtime::session_exists));
    let Some(config) = config else {
        if marker.is_some() {
            return Err(BackendError::invalid_request(
                "--build-marker requires a WinBoat target",
            ));
        }
        return Ok(None);
    };
    let build = marker.and_then(|p| BuildWatch::new(Path::new(p)).ok());
    let studio_id = runtime
        .and_then(crate::winboat::runtime::observation)
        .and_then(|(_, s)| s);
    let source = Source {
        config: config.clone(),
        management: crate::winboat::vm_use::observation(),
        runtime_id: runtime.map(str::to_owned),
        studio_id,
        build,
        build_requested: marker.is_some(),
    };
    let mut baseline = source.snapshot().await;
    let missing = [
        (Component::ManagedVm, baseline.managed_vm.is_none()),
        (Component::Container, baseline.container.is_none()),
        (Component::Compose, baseline.compose.is_none()),
        (
            Component::PublishedPorts,
            baseline.published_ports.is_none(),
        ),
        (Component::Runtime, baseline.runtime.is_none()),
        (Component::Studio, baseline.studio.is_none()),
        (Component::Build, baseline.build.is_none()),
    ]
    .into_iter()
    .filter_map(|(c, missing)| missing.then_some(c))
    .collect();
    let mut report = Report {
        version: 1,
        preparation_id: crate::contracts::secure_identifier("preparation")?,
        baseline: baseline.clone(),
        latest: baseline.clone(),
        observations: 0,
        events: vec![],
        comparable: false,
        missing,
        actor: "unknown".into(),
        interval_milliseconds: 1000,
    };
    if let Some(prepared) = preparation {
        if !prepared.valid() || prepared.interrupted() {
            return Err(observer_error());
        }
        report.preparation_id.clone_from(&prepared.preparation_id);
        report.baseline.clone_from(&prepared.baseline);
        // Watch counters are local to each observer. File metadata/content
        // identity detects changes between preparation and attachment; each
        // participant's watcher detects later identical-content writes.
        report.baseline.build_watch_generation = baseline.build_watch_generation;
        baseline.observed_at = baseline.observed_at.max(report.baseline.observed_at);
    }
    report.observe(
        baseline,
        runtime.is_some(),
        source.studio_id.is_some(),
        source.build_requested,
    );
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|_| observer_error())?;
    let address = listener.local_addr().map_err(|_| observer_error())?;
    let token = crate::contracts::secure_identifier("observation")?;
    let url = format!("http://{address}/{token}");
    let initial = report.clone();
    let worker = tokio::spawn(async move {
        // Serialize requests: one bounded inspection at a time; no unbounded tasks.
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut header = [0u8; 2048];
            let read = tokio::time::timeout(Duration::from_secs(2), async {
                let mut used = 0;
                while used < header.len() {
                    let n = stream.read(&mut header[used..]).await.ok()?;
                    if n == 0 {
                        return None;
                    }
                    used += n;
                    if header[..used].windows(4).any(|w| w == b"\r\n\r\n") {
                        return Some(used);
                    }
                }
                None
            })
            .await;
            if !matches!(read, Ok(Some(n)) if header[..n].starts_with(format!("GET /{token} HTTP/1.1\r\n").as_bytes()))
            {
                continue;
            }
            let snapshot = source.snapshot().await;
            report.observe(
                snapshot,
                source.runtime_id.is_some(),
                source.studio_id.is_some(),
                source.build_requested,
            );
            let Ok(body) = serde_json::to_vec(&report) else {
                continue;
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                stream.write_all(response.as_bytes()).await?;
                stream.write_all(&body).await
            })
            .await;
        }
    });
    Ok(Some(Observer {
        url,
        initial,
        worker,
    }))
}
fn observer_error() -> BackendError {
    BackendError::operation(
        crate::contracts::BackendId::LinuxWinboat,
        CapabilityId::BrowserTest,
        "the read-only environment observer could not start",
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod live_tests;
