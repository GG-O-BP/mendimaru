pub(crate) mod studio;
mod transport;

use crate::app_paths::AppPaths;
use crate::contracts::{
    BackendError, BackendErrorCode, BackendId, CapabilityId, RuntimeMode, RuntimeState,
    CONTRACT_SCHEMA_VERSION,
};
use crate::host_service::{failure, StartRequest, Status};
use crate::models::AppConfig;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::task::JoinSet;
use transport::CONNECT_TIMEOUT;

const START: CapabilityId = CapabilityId::HostServiceStart;
const STATUS: CapabilityId = CapabilityId::HostServiceStatus;
const STOP: CapabilityId = CapabilityId::HostServiceStop;
const MAX_STATE: u64 = 8192;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredStatus {
    record_version: u32,
    vm_key: String,
    status: Status,
}

fn fail(message: &'static str) -> BackendError {
    failure(START, message)
}
fn io_error(_: impl std::fmt::Debug) -> BackendError {
    fail("host-service private state or transport is unavailable")
}

// 디렉터리와 파일은 같은 UID만 접근한다. symlink/hardlink와 과대 레코드는
// 거부하며 stale 레코드는 프로세스 종료/포트 해제의 증거로 사용하지 않는다.
fn root() -> Result<PathBuf, BackendError> {
    let paths = AppPaths::discover_for_cli().map_err(io_error)?;
    paths.ensure_cache_directory().map_err(io_error)?;
    let path = paths.cache_directory().join("host-services");
    let secure =
        crate::downloads::storage::SecureDirectory::open_or_create(&path).map_err(io_error)?;
    let _ = secure;
    let meta = std::fs::symlink_metadata(&path).map_err(io_error)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err(fail(
            "host-service cache must be an owned private directory",
        ));
    }
    Ok(path)
}

fn socket_path(root: &Path, id: &str) -> PathBuf {
    // 짧은 경로로 Linux sockaddr_un 한도를 지킨다. 128-bit ID는 레코드에도 검증한다.
    root.join(format!("{}.sock", &id[8..]))
}

fn read_status(config: &AppConfig, root: &Path, id: &str) -> Result<Status, BackendError> {
    if !crate::host_service::valid_id(id, "hostsvc_") {
        return Err(BackendError::invalid_request(
            "invalid host-service session ID",
        ));
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(root.join(format!("{id}.json")))
        .map_err(io_error)?;
    let meta = file.metadata().map_err(io_error)?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_STATE
    {
        return Err(fail("host-service state is untrusted"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_STATE + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let stored: StoredStatus = serde_json::from_slice(&bytes).map_err(io_error)?;
    if stored.record_version != 1
        || stored.vm_key != super::vm_use::identity_key(config).map_err(io_error)?
    {
        return Err(fail(
            "host-service state belongs to another VM or record version",
        ));
    }
    let mut state = stored.status;
    if state.session_id != id
        || state.backend != BackendId::LinuxWinboat
        || !matches!(
            state.schema_version.as_str(),
            "7.0.0" | CONTRACT_SCHEMA_VERSION
        )
        || state.service != "mysql"
        || state.guest_host != "127.0.0.1"
        || state.target_host != "127.0.0.1"
        || state.target_port != 3306
        || state.guest_port == 0
        || !crate::host_service::valid_id(&state.runtime_session_id, "runtime_")
        || !state.encrypted
        || state.database_authentication_verified
        || state.web_host_port.is_some_and(|p| p != 18080)
        || state.admin_host_port.is_some_and(|p| p != 18090)
        || state.active_connections > 64
        || !matches!(state.state.as_str(), "ready" | "stopped" | "interrupted")
        || (state.state == "ready"
            && !(state.guest_listener_verified
                && state.target_tcp_verified
                && state.mysql_handshake_verified))
        || (state.state != "ready"
            && (state.guest_listener_verified
                || state.target_tcp_verified
                || state.mysql_handshake_verified
                || state.active_connections != 0))
    {
        return Err(fail("host-service state identity is incompatible"));
    }
    state.schema_version = CONTRACT_SCHEMA_VERSION.into();
    Ok(state)
}

fn save(config: &AppConfig, root: &Path, status: &Status) -> Result<(), BackendError> {
    let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(io_error)?;
    let stored = StoredStatus {
        record_version: 1,
        vm_key: super::vm_use::identity_key(config).map_err(io_error)?,
        status: status.clone(),
    };
    let bytes = serde_json::to_vec(&stored).map_err(io_error)?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(io_error)?;
    temporary
        .persist(root.join(format!("{}.json", status.session_id)))
        .map_err(io_error)?;
    std::fs::File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(io_error)
}

struct PendingChild {
    child: tokio::process::Child,
    accepted: bool,
}
impl Drop for PendingChild {
    fn drop(&mut self) {
        if !self.accepted {
            let _ = self.child.start_kill();
        }
    }
}

pub(crate) async fn start(
    config: &AppConfig,
    request: &StartRequest,
) -> Result<Status, BackendError> {
    request.validate()?;
    // 같은 Runtime의 재실행은 이미 인증된 연결을 재사용한다. 시작 경쟁은
    // keeper 수명 전체의 flock으로 막고 stale 레코드는 보존한다.
    for status in list(config).await? {
        if status.runtime_session_id == request.runtime_session_id && status.state == "ready" {
            // An environment value can rotate without changing its name. Never
            // silently reuse an existing authentication binding.
            if request.m2ee_password_env.is_some() || status.m2ee_authentication_configured {
                return Err(fail("M2EE authentication binding already exists; stop the owned host-service before restarting"));
            }
            if request.guest_port != 0 && request.guest_port != status.guest_port {
                return Err(fail(
                    "host-service already owns a different guest port for this Runtime",
                ));
            }
            return Ok(status);
        }
    }
    if let Some(name) = &request.m2ee_password_env {
        let secret = zeroize::Zeroizing::new(std::env::var(name).map_err(|_| {
            fail("M2EE password environment variable must contain a nonempty UTF-8 value")
        })?);
        if !crate::host_service::valid_m2ee_password(&secret) {
            return Err(fail("M2EE password environment variable must contain 1 to 256 UTF-8 bytes without control characters"));
        }
    }
    let mut command = tokio::process::Command::new(std::env::current_exe().map_err(io_error)?);
    command
        .arg("__host-service-keeper")
        .arg(&request.runtime_session_id)
        .arg(request.guest_port.to_string())
        .arg(request.m2ee_password_env.as_deref().unwrap_or(""));
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut pending = PendingChild {
        child: command.spawn().map_err(io_error)?,
        accepted: false,
    };
    let mut output = BufReader::new(
        pending
            .child
            .stdout
            .take()
            .ok_or_else(|| fail("host-service keeper handshake unavailable"))?,
    )
    .take(MAX_STATE);
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(60),
        output.read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(io_error)?
    .map_err(io_error)?;
    let result: Result<Status, BackendError> = serde_json::from_slice(&bytes).map_err(io_error)?;
    let status = result?;
    let mut input = pending
        .child
        .stdin
        .take()
        .ok_or_else(|| fail("host-service keeper acknowledgement unavailable"))?;
    input.write_all(b"accept\n").await.map_err(io_error)?;
    input.flush().await.map_err(io_error)?;
    pending.accepted = true;
    Ok(status)
}

pub(crate) async fn inspect(
    config: &AppConfig,
    id: &str,
    stop: bool,
) -> Result<Status, BackendError> {
    let root = root()?;
    let mut status = read_status(config, &root, id)?;
    if status.state == "stopped" {
        return Ok(status);
    }
    let path = socket_path(&root, id);
    match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&path)).await {
        Ok(Ok(mut ipc)) => {
            if ipc.peer_cred().map_err(io_error)?.uid() != unsafe { libc::geteuid() } {
                return Err(fail("host-service keeper owner mismatch"));
            }
            ipc.write_all(&[u8::from(stop)]).await.map_err(io_error)?;
            let mut response = Vec::new();
            tokio::time::timeout(
                Duration::from_secs(180),
                ipc.take(MAX_STATE).read_to_end(&mut response),
            )
            .await
            .map_err(io_error)?
            .map_err(io_error)?;
            let response: Result<Status, BackendError> =
                serde_json::from_slice(&response).map_err(io_error)?;
            let response = response?;
            if response.session_id != id {
                return Err(fail("host-service keeper session mismatch"));
            }
            Ok(response)
        }
        Ok(Err(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            status.state = "interrupted".into();
            status.guest_listener_verified = false;
            status.target_tcp_verified = false;
            status.mysql_handshake_verified = false;
            status.active_connections = 0;
            // 죽은 keeper를 PID로 kill하거나 다른 사용자의 포트를 해제하지 않는다.
            Ok(status)
        }
        _ => Err(failure(
            if stop { STOP } else { STATUS },
            "host-service keeper is unavailable; retry status",
        )),
    }
}

pub(crate) async fn list(config: &AppConfig) -> Result<Vec<Status>, BackendError> {
    let root = root()?;
    let mut results = Vec::new();
    for (count, entry) in std::fs::read_dir(&root).map_err(io_error)?.enumerate() {
        if count >= 4096 {
            return Err(failure(
                STATUS,
                "host-service cache exceeds the bounded record limit; remove stopped records",
            ));
        }
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|s| s.strip_suffix(".json"))
            .filter(|s| crate::host_service::valid_id(s, "hostsvc_"))
        else {
            continue;
        };
        // 변조/손상 레코드는 사용자가 진단 후 격리한다. 자동 삭제하지 않는다.
        results.push(inspect(config, id, false).await?);
    }
    results.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(results)
}

fn runtime_lock(root: &Path, runtime: &str) -> Result<std::fs::File, BackendError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(root.join(format!("{runtime}.lock")))
        .map_err(io_error)?;
    let meta = file.metadata().map_err(io_error)?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err(fail("host-service Runtime lock is untrusted"));
    }
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        let mut error = fail("host-service start is already owned for this Runtime; retry status");
        error.code = BackendErrorCode::PreconditionFailed;
        error.retryable = true;
        error
    })?;
    Ok(file)
}

pub(crate) fn keeper_dispatch(arguments: &[std::ffi::OsString]) -> i32 {
    let result = crate::i18n::initialize("en-US")
        .map_err(io_error)
        .and_then(|_| {
            let [runtime, port, password_env] = arguments else {
                return Err(BackendError::invalid_request(
                    "invalid host-service keeper request",
                ));
            };
            let request = StartRequest {
                runtime_session_id: runtime.to_str().unwrap_or_default().into(),
                service: "mysql".into(),
                allow_host_port: 3306,
                guest_port: port
                    .to_str()
                    .unwrap_or_default()
                    .parse()
                    .map_err(io_error)?,
                m2ee_password_env: password_env
                    .to_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            };
            request.validate()?;
            let paths = AppPaths::discover_for_cli().map_err(io_error)?;
            let config = crate::config::load_config_from(&paths).map_err(io_error)?;
            tauri::async_runtime::block_on(keeper(&config, &request))
        });
    if let Err(error) = result {
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            serde_json::to_string(&Result::<Status, _>::Err(error)).unwrap_or_default()
        );
        return 1;
    }
    0
}

struct Cleanup {
    socket: PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

async fn keeper(config: &AppConfig, request: &StartRequest) -> Result<(), BackendError> {
    let _runtime_lock = runtime_lock(&root()?, &request.runtime_session_id)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(io_error)?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(io_error)?;
    let runtime = super::runtime::status(config, &request.runtime_session_id).await?;
    if runtime.mode != RuntimeMode::StudioRunLocally
        || matches!(runtime.state, RuntimeState::Stopped | RuntimeState::Failed)
    {
        return Err(fail(
            "host-service requires an active Studio Run Locally Runtime session",
        ));
    }
    let prepared = prepare(config, request).await?;
    let (mut status, root, ipc, listener, acceptor, security, _cleanup) = prepared;
    // Linux loopback에만 바인딩하며 Docker의 기존 NIC 전달과 포트를 분리한다.
    // 점유된 포트는 탈취하지 않고 시작을 실패시킨다.
    let web = TcpListener::bind((Ipv4Addr::LOCALHOST, 18080))
        .await
        .map_err(io_error)?;
    let admin = TcpListener::bind((Ipv4Addr::LOCALHOST, 18090))
        .await
        .map_err(io_error)?;
    let (web_tx, web_rx) = tokio::sync::mpsc::channel(16);
    let (admin_tx, admin_rx) = tokio::sync::mpsc::channel(4);
    let key = Arc::new(zeroize::Zeroizing::new(security.host_service_key()));
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, 3306));
    let active = Arc::new(AtomicUsize::new(0));
    let mut streams = JoinSet::new();
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(1);
    // 상위 JoinSet을 drop/종료하면 리스너와 하위 TLS 연결을 함께 취소한다.
    streams.spawn(transport::serve_loopback(web, web_rx));
    streams.spawn(transport::serve_loopback(admin, admin_rx));
    streams.spawn(transport::serve_reverse(
        listener,
        acceptor,
        key,
        target,
        transport::Channels {
            ready: ready_tx,
            reverse: Some([web_tx, admin_tx]),
        },
        active.clone(),
    ));
    status.guest_port = tokio::time::timeout(Duration::from_secs(20), ready_rx.recv())
        .await
        .map_err(io_error)?
        .filter(|p| *p != 0)
        .ok_or_else(|| fail("host-service guest listener did not authenticate"))?;
    if request.guest_port != 0 && status.guest_port != request.guest_port {
        return Err(fail("host-service guest port mismatch"));
    }
    status.guest_listener_verified = true;
    status.target_tcp_verified = true;
    status.mysql_handshake_verified = true;
    status.web_host_port = Some(18080);
    status.admin_host_port = Some(18090);
    save(config, &root, &status)?;
    writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string(&Result::<_, BackendError>::Ok(&status)).map_err(io_error)?
    )
    .map_err(io_error)?;
    std::io::stdout().flush().map_err(io_error)?;
    let mut acknowledgement = Vec::new();
    let count = tokio::time::timeout(
        CONNECT_TIMEOUT,
        BufReader::new(tokio::io::stdin())
            .take(16)
            .read_until(b'\n', &mut acknowledgement),
    )
    .await
    .map_err(io_error)?
    .map_err(io_error)?;
    if count == 0 || acknowledgement != b"accept\n" {
        return Err(fail("host-service launch was not accepted"));
    }
    let mut poll = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            incoming = ipc.accept() => {
                let (mut client, _) = incoming.map_err(io_error)?;
                if client.peer_cred().map_err(io_error)?.uid() != unsafe { libc::geteuid() } { continue; }
                let Ok(Ok(action)) = tokio::time::timeout(Duration::from_secs(1), client.read_u8()).await else { continue; };
                status.active_connections = active.load(Ordering::Relaxed);
                let result = if action == 1 {
                    stop_dependency(config, &status.runtime_session_id).await.map(|()| { mark_inactive(&mut status, "stopped"); status.clone() })
                } else if action == 0 { Ok(status.clone()) } else { Err(BackendError::invalid_request("invalid host-service control")) };
                if status.state == "stopped" { streams.shutdown().await; save(config, &root, &status)?; }
                let bytes = serde_json::to_vec(&result).map_err(io_error)?;
                let _ = tokio::time::timeout(CONNECT_TIMEOUT, client.write_all(&bytes)).await;
                if status.state == "stopped" { break; }
            }
            _ = poll.tick() => {
                if ready_rx.try_recv().is_ok() {
                    status.state = "interrupted".into(); break;
                }
                // HTTP가 아직 열리지 않은 동안 status의 네트워크 probe를 반복하면
                // shared VM lease가 UI 입력의 exclusive lease를 굶길 수 있다.
                // 불변 소유권 레코드만 관찰하고 VM/helper 손실은 TLS heartbeat로 감지한다.
                if super::runtime::observation(&status.runtime_session_id).is_none() {
                    status.state = "stopped".into(); break;
                }
            }
            _ = interrupt.recv() => { status.state = "interrupted".into(); break; }
            _ = terminate.recv() => { status.state = "interrupted".into(); break; }
            _ = streams.join_next() => { status.state = "interrupted".into(); break; }
        }
    }
    streams.shutdown().await;
    let terminal = status.state.clone();
    mark_inactive(&mut status, &terminal);
    save(config, &root, &status)?;
    Ok(())
}

fn mark_inactive(status: &mut Status, state: &str) {
    status.state = state.into();
    status.guest_listener_verified = false;
    status.target_tcp_verified = false;
    status.mysql_handshake_verified = false;
    status.active_connections = 0;
}

async fn stop_dependency(config: &AppConfig, id: &str) -> Result<(), BackendError> {
    // readiness refresh는 host-service IPC를 조회하므로 이 keeper 안에서는 순환 대기가 된다.
    // 저장된 세션 소유권을 읽고, 아래의 인증된 Studio 종료 확인은 그대로 수행한다.
    let runtime = super::runtime::recorded_status_for_stop(id)?;
    // 정상 Studio 종료가 취소되거나 저장 대화상자가 남으면 DB 경로를 유지한다.
    if let Some(studio) = &runtime.studio_session_id {
        let paths = AppPaths::discover_for_cli().map_err(io_error)?;
        let refused = || {
            failure(
                STOP,
                "dependent Studio session did not confirm graceful exit; host-service preserved",
            )
        };
        let stopped = crate::cli::request_keeper_stop(&paths, studio)
            .await
            .map_err(|_| refused())?;
        if !stopped {
            // keeper가 없다고 별도 RDP를 열면 다른 RemoteApp과 DB helper가
            // 먼저 끊길 수 있다. 정확한 기존 채널이 정상 종료를 확인해야 한다.
            return Err(refused());
        }
    } else if runtime.http_ready {
        let mut error = failure(
            STOP,
            "stop the dependent Run Locally app through Studio before stopping its host-service",
        );
        error.code = BackendErrorCode::PreconditionFailed;
        return Err(error);
    }
    if runtime.state == RuntimeState::Stopped {
        Ok(())
    } else {
        super::runtime::stop(config, id).await
    }
}

type Prepared = (
    Status,
    PathBuf,
    UnixListener,
    TcpListener,
    tokio_rustls::TlsAcceptor,
    super::security::OperationSecurity,
    Cleanup,
);

async fn prepare(config: &AppConfig, request: &StartRequest) -> Result<Prepared, BackendError> {
    let studio = super::runtime::observation(&request.runtime_session_id)
        .and_then(|(_, studio)| studio)
        .ok_or_else(|| {
            fail("host-service requires a Runtime linked to a managed Studio session")
        })?;
    super::container::ensure_private_operation_transport(config)
        .await
        .map_err(io_error)?;
    let gateway = bridge_gateway(config).await?;
    let listener = TcpListener::bind((gateway, 0)).await.map_err(io_error)?;
    let relay_port = listener.local_addr().map_err(io_error)?.port();
    let probe = tokio::time::timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((Ipv4Addr::LOCALHOST, 3306)),
    )
    .await
    .map_err(io_error)?
    .map_err(io_error)?;
    drop(probe);
    let (tls, pin) = transport::tls_server().map_err(io_error)?;
    let id = crate::contracts::secure_identifier("hostsvc")?;
    let root = root()?;
    let socket = socket_path(&root, &id);
    let ipc = UnixListener::bind(&socket).map_err(io_error)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    let cleanup = Cleanup {
        socket: socket.clone(),
    };
    use sha2::{Digest, Sha256};
    let security = super::security::OperationSecurity::generate(&format!(
        "{:x}",
        Sha256::digest(include_bytes!("../../scripts/host_service.cs"))
    ))
    .map_err(io_error)?;
    let packet = self::studio::Launch {
        id: id.clone(),
        host: gateway,
        port: relay_port,
        pin: pin.clone(),
        guest_port: request.guest_port,
        operation_key: security.bootstrap_key_text().to_string(),
        m2ee_password: request
            .m2ee_password_env
            .as_ref()
            .map(|name| {
                std::env::var(name)
                    .map_err(|_| fail("M2EE password environment variable is unavailable"))
            })
            .transpose()?,
    };
    let paths = AppPaths::discover_for_cli().map_err(io_error)?;
    crate::cli::request_keeper_host_service(&paths, &studio, &packet).await?;
    Ok((
        Status {
            schema_version: CONTRACT_SCHEMA_VERSION.into(),
            session_id: id,
            runtime_session_id: request.runtime_session_id.clone(),
            backend: BackendId::LinuxWinboat,
            service: "mysql".into(),
            state: "ready".into(),
            guest_host: "127.0.0.1".into(),
            guest_port: 0,
            target_host: "127.0.0.1".into(),
            target_port: 3306,
            guest_listener_verified: false,
            target_tcp_verified: false,
            mysql_handshake_verified: false,
            database_authentication_verified: false,
            encrypted: true,
            active_connections: 0,
            web_host_port: None,
            admin_host_port: None,
            m2ee_authentication_configured: request.m2ee_password_env.is_some(),
        },
        root,
        ipc,
        listener,
        tls,
        security,
        cleanup,
    ))
}

async fn bridge_gateway(config: &AppConfig) -> Result<Ipv4Addr, BackendError> {
    let mut command = tokio::process::Command::new(config.container_runtime.as_str());
    command
        .arg("inspect")
        .arg(&config.container_name)
        .args(["--format", "{{json .NetworkSettings.Networks}}"]);
    let output = crate::process::output(
        command,
        crate::process::CommandPolicy::STATUS,
        None,
        "host-service bridge inspection",
    )
    .await
    .map_err(io_error)?;
    if !output.status.success() {
        return Err(fail("host-service bridge inspection failed"));
    }
    gateway_from_inspection(&output.stdout)
}

fn gateway_from_inspection(bytes: &[u8]) -> Result<Ipv4Addr, BackendError> {
    let networks: Value = serde_json::from_slice(bytes).map_err(io_error)?;
    let networks = networks
        .as_object()
        .filter(|n| n.len() == 1)
        .ok_or_else(|| fail("host-service requires one private WinBoat bridge network"))?;
    let network = networks.values().next().unwrap();
    let gateway: Ipv4Addr = network["Gateway"]
        .as_str()
        .unwrap_or_default()
        .parse()
        .map_err(io_error)?;
    let address: Ipv4Addr = network["IPAddress"]
        .as_str()
        .unwrap_or_default()
        .parse()
        .map_err(io_error)?;
    let prefix = network["IPPrefixLen"]
        .as_u64()
        .filter(|p| (8..=30).contains(p))
        .ok_or_else(|| fail("invalid WinBoat bridge prefix"))? as u32;
    let mask = u32::MAX << (32 - prefix);
    if !gateway.is_private()
        || !address.is_private()
        || gateway == address
        || u32::from(gateway) & mask != u32::from(address) & mask
    {
        return Err(fail(
            "host-service refuses public, host-mode, or mismatched bridge routes",
        ));
    }
    Ok(gateway)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AppConfig {
        serde_json::from_value(serde_json::json!({
            "winboatExecutable":"fixture", "composeFile":"/missing/compose.yaml",
            "containerRuntime":"docker", "containerName":"host-service-fixture",
            "apiUrl":"http://127.0.0.1:9", "rdpHost":"127.0.0.1", "rdpPort":9,
            "sharedDirectory":"/missing", "windowsSharedDirectory":"fixture",
            "freerdpBinary":"fixture", "mendixInstallRoot":"fixture", "mendixDataRoot":"fixture",
            "startupTimeoutSeconds":1
        }))
        .unwrap()
    }

    fn state() -> Status {
        Status {
            schema_version: CONTRACT_SCHEMA_VERSION.into(),
            session_id: format!("hostsvc_{}", "a".repeat(32)),
            runtime_session_id: format!("runtime_{}", "b".repeat(32)),
            backend: BackendId::LinuxWinboat,
            service: "mysql".into(),
            state: "ready".into(),
            guest_host: "127.0.0.1".into(),
            guest_port: 33306,
            target_host: "127.0.0.1".into(),
            target_port: 3306,
            guest_listener_verified: true,
            target_tcp_verified: true,
            mysql_handshake_verified: true,
            database_authentication_verified: false,
            encrypted: true,
            active_connections: 0,
            web_host_port: None,
            admin_host_port: None,
            m2ee_authentication_configured: false,
        }
    }

    #[test]
    fn v7_host_service_records_remain_readable_without_rewriting_state() {
        let directory = tempfile::tempdir().unwrap();
        let config = config();
        let mut status = state();
        status.schema_version = "7.0.0".into();
        save(&config, directory.path(), &status).unwrap();
        let path = directory.path().join(format!("{}.json", status.session_id));
        let before = std::fs::read(&path).unwrap();
        let loaded = read_status(&config, directory.path(), &status.session_id).unwrap();
        assert_eq!(loaded.schema_version, CONTRACT_SCHEMA_VERSION);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        status.schema_version = "9.0.0".into();
        save(&config, directory.path(), &status).unwrap();
        assert!(read_status(&config, directory.path(), &status.session_id).is_err());
    }

    #[test]
    fn records_reject_another_vm_identity_indirection_permissions_and_unbounded_input() {
        let directory = tempfile::tempdir().unwrap();
        let config = config();
        let status = state();
        let root = directory.path();
        save(&config, root, &status).unwrap();
        assert!(read_status(&config, root, &status.session_id).is_ok());
        let mut other = config.clone();
        other.container_name = "another-vm".into();
        assert!(read_status(&other, root, &status.session_id).is_err());
        let path = root.join(format!("{}.json", status.session_id));
        let hardlink = root.join("hardlink");
        std::fs::hard_link(&path, &hardlink).unwrap();
        assert!(read_status(&config, root, &status.session_id).is_err());
        std::fs::remove_file(hardlink).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_status(&config, root, &status.session_id).is_err());
        save(&config, root, &status).unwrap();
        let moved = root.join("original");
        std::fs::rename(&path, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &path).unwrap();
        assert!(read_status(&config, root, &status.session_id).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(moved, &path).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_STATE + 1)
            .unwrap();
        assert!(read_status(&config, root, &status.session_id).is_err());
        save(&config, root, &status).unwrap();
        let mut wrong = status.clone();
        wrong.session_id = format!("hostsvc_{}", "c".repeat(32));
        let stored = StoredStatus {
            record_version: 1,
            vm_key: super::super::vm_use::identity_key(&config).unwrap(),
            status: wrong,
        };
        std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(read_status(&config, root, &status.session_id).is_err());
    }

    #[test]
    fn records_reject_false_readiness_authentication_and_destination_claims() {
        let directory = tempfile::tempdir().unwrap();
        let config = config();
        for change in 0..8 {
            let mut status = state();
            match change {
                0 => status.runtime_session_id = "another-runtime".into(),
                1 => status.encrypted = false,
                2 => status.database_authentication_verified = true,
                3 => status.mysql_handshake_verified = false,
                4 => status.state = "stopped".into(),
                5 => status.target_host = "192.0.2.1".into(),
                6 => status.web_host_port = Some(8080),
                _ => status.admin_host_port = Some(8090),
            }
            save(&config, directory.path(), &status).unwrap();
            assert!(read_status(&config, directory.path(), &status.session_id).is_err());
        }
        let mut status = state();
        status.active_connections = 3;
        mark_inactive(&mut status, "stopped");
        save(&config, directory.path(), &status).unwrap();
        assert!(read_status(&config, directory.path(), &status.session_id).is_ok());
    }

    #[test]
    fn legacy_host_service_records_and_reverse_http_ports_remain_readable() {
        let directory = tempfile::tempdir().unwrap();
        let config = config();
        let mut status = state();
        save(&config, directory.path(), &status).unwrap();
        let path = directory.path().join(format!("{}.json", status.session_id));
        let original = std::fs::read(&path).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert!(stored["status"].get("webHostPort").is_none());
        assert!(stored["status"].get("adminHostPort").is_none());
        let restored = read_status(&config, directory.path(), &status.session_id).unwrap();
        assert_eq!(restored.web_host_port, None);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        status.web_host_port = Some(18080);
        status.admin_host_port = Some(18090);
        save(&config, directory.path(), &status).unwrap();
        let restored = read_status(&config, directory.path(), &status.session_id).unwrap();
        assert_eq!(restored.web_host_port, Some(18080));
        assert_eq!(restored.admin_host_port, Some(18090));
    }

    #[test]
    fn runtime_lock_serializes_owners_and_rejects_untrusted_lock_files() {
        let directory = tempfile::tempdir().unwrap();
        let id = state().runtime_session_id;
        let lock = runtime_lock(directory.path(), &id).unwrap();
        assert!(runtime_lock(directory.path(), &id).is_err());
        drop(lock);
        assert!(runtime_lock(directory.path(), &id).is_ok());
        let path = directory.path().join(format!("{id}.lock"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(runtime_lock(directory.path(), &id).is_err());
        std::fs::remove_file(&path).unwrap();
        let target = directory.path().join("user-file");
        std::fs::write(&target, b"preserved").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(runtime_lock(directory.path(), &id).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"preserved");
    }

    #[tokio::test]
    #[ignore = "requires live WinBoat, MySQL 8.4, and an explicitly selected existing Runtime"]
    async fn live_mysql_greeting_ownership_and_control_loss_cleanup() {
        assert_eq!(
            std::env::var("MENDIMARU_HOST_SERVICE_LIVE").as_deref(),
            Ok("1")
        );
        crate::i18n::initialize("en-US").unwrap();
        let config = crate::config::detect_config().unwrap();
        let runtime = std::env::var("MENDIMARU_HOST_SERVICE_RUNTIME_ID")
            .expect("select an existing Runtime explicitly");
        let mut request = StartRequest {
            runtime_session_id: runtime,
            service: "mysql".into(),
            allow_host_port: 3306,
            guest_port: 0,
            m2ee_password_env: None,
        };
        request.validate().unwrap();
        let compose = std::fs::read(&config.compose_file).unwrap();
        let (mut first, _, _ipc, listener, acceptor, security, cleanup) =
            prepare(&config, &request).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let mut tasks = JoinSet::new();
        tasks.spawn(transport::serve(
            listener,
            acceptor,
            Arc::new(zeroize::Zeroizing::new(security.host_service_key())),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 3306)),
            tx,
            Arc::new(AtomicUsize::new(0)),
        ));
        first.guest_port = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(first.guest_port > 0);
        request.guest_port = first.guest_port;
        // 두 번째 helper의 충돌이 첫 리스너/제어 채널을 정리해서는 안 된다.
        assert!(prepare(&config, &request).await.is_err());
        assert!(rx.try_recv().is_err());
        tasks.shutdown().await;
        // heartbeat 단절 후 helper가 소유한 리스너를 해제하고 monitor가
        // 완료 작업을 수거할 시간을 준다. 재시작 성공이 포트 재사용을 증명한다.
        tokio::time::sleep(Duration::from_secs(3)).await;
        drop(cleanup);
        let (_, _, _, listener, acceptor, security, cleanup) =
            prepare(&config, &request).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tasks.spawn(transport::serve(
            listener,
            acceptor,
            Arc::new(zeroize::Zeroizing::new(security.host_service_key())),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 3306)),
            tx,
            Arc::new(AtomicUsize::new(0)),
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(20), rx.recv())
                .await
                .unwrap()
                .unwrap(),
            first.guest_port
        );
        tasks.shutdown().await;
        drop(cleanup);
        assert_eq!(std::fs::read(&config.compose_file).unwrap(), compose);
        println!("PASS: real Windows loopback to Linux MySQL greeting, exclusive guest port, authenticated control loss, owned port reuse, and unchanged Compose; no DB authentication or app acceptance claimed");
    }
    #[test]
    fn gateway_is_dynamic_private_single_network_and_same_subnet() {
        let valid = br#"{"vm":{"Gateway":"172.20.0.1","IPAddress":"172.20.0.9","IPPrefixLen":24}}"#;
        assert_eq!(
            gateway_from_inspection(valid).unwrap(),
            Ipv4Addr::new(172, 20, 0, 1)
        );
        for invalid in [
            "{}",
            "{\"host\":{}}",
            "{\"a\":{},\"b\":{}}",
            "{\"vm\":{\"Gateway\":\"0.0.0.0\",\"IPAddress\":\"172.20.0.9\",\"IPPrefixLen\":24}}",
            "{\"vm\":{\"Gateway\":\"172.21.0.1\",\"IPAddress\":\"172.20.0.9\",\"IPPrefixLen\":24}}",
        ] {
            assert!(gateway_from_inspection(invalid.as_bytes()).is_err());
        }
    }
}
