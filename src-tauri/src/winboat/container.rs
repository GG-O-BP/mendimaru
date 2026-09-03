use crate::config::{
    compose_file_is_valid, compose_shared_directory, path_exists_or_binary,
    resolved_winboat_executable, runtime_host_port_async, winboat_compose_service_name,
};
use crate::models::{
    AppConfig, ContainerRuntime, ContainerStatus, EnvironmentDiagnostic,
    EnvironmentDiagnosticAction, EnvironmentDiagnosticErrorCode, EnvironmentDiagnosticId,
    EnvironmentDiagnosticStatus, EnvironmentStatus,
};
use crate::process::{self, CommandFailure, CommandFailureKind, CommandOutput, CommandPolicy};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;
use tokio::process::Command;

pub async fn environment_status(config: &AppConfig) -> EnvironmentStatus {
    let winboat_available = resolved_winboat_executable(config).is_some();
    let compose_available = Path::new(&config.compose_file).is_file();
    let compose_valid = compose_available && compose_file_is_valid(&config.compose_file);
    let browser_executable = crate::marketplace::browser_executable();
    let browser_probe = async {
        match browser_executable.as_deref() {
            Some(executable) => probe_tool(executable, &["--version"]).await,
            None => ToolProbe::default(),
        }
    };
    let (runtime, freerdp, browser, inspection) = tokio::join!(
        probe_container_runtime(config.container_runtime.as_str()),
        probe_tool(&config.freerdp_binary, &["/version"]),
        browser_probe,
        inspect_container(config),
    );
    #[cfg(target_os = "linux")]
    let browser_sandbox_available = if browser.usable {
        crate::marketplace::browser_sandbox_available().await
    } else {
        false
    };
    #[cfg(not(target_os = "linux"))]
    let browser_sandbox_available = browser.usable;
    let shared_directory_available = Path::new(&config.shared_directory).is_dir();
    let compose_shared = compose_shared_directory(&config.compose_file);
    let current_shared = if inspection.status.exists() {
        inspection.shared_directory.as_deref()
    } else {
        compose_shared.as_deref()
    };
    let shared_mount_matches = compose_available
        && current_shared
            .is_some_and(|current| paths_refer_to_same_location(current, &config.shared_directory));
    let container_status = inspection.status;
    let winboat_initialized = compose_available && container_status.exists();
    let mut guest_online = false;
    let mut guest_error_code = None;
    let mut guest_clock = None;
    let mut rdp_reachable = false;
    let mut rdp_error_code = None;
    if winboat_initialized && container_status.is_running() {
        let (api_port, rdp_port) = tokio::join!(
            runtime_host_port_async(config, 7148, "tcp"),
            runtime_host_port_async(config, 3389, "tcp"),
        );
        let api_url = match api_port {
            Ok(Some(port)) => format!("http://127.0.0.1:{port}"),
            Ok(None) => config.api_url.clone(),
            Err(error) => {
                guest_error_code = Some(environment_error_code(error.kind()));
                config.api_url.clone()
            }
        };
        let rdp_port = match rdp_port {
            Ok(Some(port)) => port,
            Ok(None) => config.rdp_port,
            Err(error) => {
                rdp_error_code = Some(environment_error_code(error.kind()));
                config.rdp_port
            }
        };
        let rdp_host = config.rdp_host.clone();
        let (guest_probe, rdp_probe) = tokio::join!(
            guest_api_probe(&api_url),
            tokio::task::spawn_blocking(move || tcp_endpoint_reachable(&rdp_host, rdp_port)),
        );
        guest_online = guest_probe.online && guest_error_code.is_none();
        guest_clock = if guest_online {
            guest_probe.clock
        } else {
            None
        };
        match rdp_probe {
            Ok(reachable) => rdp_reachable = reachable && rdp_error_code.is_none(),
            Err(_) => {
                rdp_error_code = Some(EnvironmentDiagnosticErrorCode::ExternalProcessInterrupted);
            }
        }
    }
    let state = LinuxDiagnosticState {
        winboat_available,
        compose_available,
        compose_valid,
        runtime,
        freerdp,
        shared_directory_available,
        shared_mount_matches,
        container_status,
        container_error_code: inspection.error_code,
        guest_online,
        guest_error_code,
        guest_clock,
        rdp_reachable,
        rdp_error_code,
        browser,
        browser_sandbox_available,
    };
    let ready = state.ready();
    let diagnostics = build_linux_diagnostics(&state);

    EnvironmentStatus {
        platform: crate::platform::capabilities(),
        ready,
        winboat_available,
        winboat_initialized,
        setup_pending: config.winboat_setup_pending,
        compose_available,
        runtime_available: state.runtime.usable,
        freerdp_available: state.freerdp.usable,
        shared_directory_available,
        shared_mount_matches,
        container_status,
        guest_online,
        diagnostics,
    }
}

#[derive(Debug, Clone, Default)]
struct ToolProbe {
    available: bool,
    usable: bool,
    version: Option<String>,
    error_code: Option<EnvironmentDiagnosticErrorCode>,
}

#[derive(Debug, Clone)]
struct LinuxDiagnosticState {
    winboat_available: bool,
    compose_available: bool,
    compose_valid: bool,
    runtime: ToolProbe,
    freerdp: ToolProbe,
    shared_directory_available: bool,
    shared_mount_matches: bool,
    container_status: ContainerStatus,
    container_error_code: Option<EnvironmentDiagnosticErrorCode>,
    guest_online: bool,
    guest_error_code: Option<EnvironmentDiagnosticErrorCode>,
    guest_clock: Option<GuestClockProbe>,
    rdp_reachable: bool,
    rdp_error_code: Option<EnvironmentDiagnosticErrorCode>,
    browser: ToolProbe,
    browser_sandbox_available: bool,
}

/// Result of the bounded Guest API health probe, including the guest clock
/// observation derived from the HTTP response.
#[derive(Debug, Clone, Default)]
struct GuestApiProbe {
    online: bool,
    clock: Option<GuestClockProbe>,
}

/// Guest clock observation derived from the Guest API HTTP `Date` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestClockProbe {
    /// The guest responded, but its clock could not be measured.
    Unmeasurable,
    /// Guest-minus-host skew in whole seconds.
    Measured(i64),
}

impl GuestClockProbe {
    fn skew_seconds(self) -> Option<i64> {
        match self {
            Self::Unmeasurable => None,
            Self::Measured(skew_seconds) => Some(skew_seconds),
        }
    }
}

const GUEST_CLOCK_SKEW_SUCCESS_SECONDS: i64 = 5;
const GUEST_CLOCK_SKEW_WARNING_SECONDS: i64 = 60;
const HOST_CLOCKSOURCE_PATH: &str =
    "/sys/devices/system/clocksource/clocksource0/current_clocksource";
const MAX_HOST_CLOCKSOURCE_BYTES: u64 = 64;

impl LinuxDiagnosticState {
    fn ready(&self) -> bool {
        self.winboat_available
            && self.compose_valid
            && self.runtime.usable
            && self.freerdp.usable
            && self.shared_directory_available
            && self.shared_mount_matches
            && self.container_status.is_running()
            && self.guest_online
            && self.rdp_reachable
    }
}

fn build_linux_diagnostics(state: &LinuxDiagnosticState) -> Vec<EnvironmentDiagnostic> {
    use EnvironmentDiagnosticAction::{OpenSettings, OpenWinboat, Redetect, StartWinboat};
    use EnvironmentDiagnosticId::{
        Compose, Container, ContainerRuntime, Freerdp, GuestApi, GuestClock, MarketplaceBrowser,
        Rdp, SharedDirectory, SharedMount, Winboat,
    };
    use EnvironmentDiagnosticStatus::{Failure, Success, Warning};

    vec![
        diagnostic(
            Winboat,
            if state.winboat_available {
                Success
            } else {
                Failure
            },
            None,
            (!state.winboat_available).then_some(Redetect),
            None,
        ),
        diagnostic(
            Compose,
            if state.compose_valid {
                Success
            } else {
                Failure
            },
            state.compose_available.then(|| {
                if state.compose_valid {
                    "valid"
                } else {
                    "invalid"
                }
                .to_string()
            }),
            (!state.compose_valid).then_some(if state.compose_available {
                OpenSettings
            } else {
                Redetect
            }),
            None,
        ),
        diagnostic(
            ContainerRuntime,
            if state.runtime.usable {
                Success
            } else {
                Failure
            },
            observed_tool(&state.runtime),
            (!state.runtime.usable).then_some(OpenSettings),
            state.runtime.error_code,
        ),
        diagnostic(
            Freerdp,
            if state.freerdp.usable {
                Success
            } else {
                Failure
            },
            observed_tool(&state.freerdp),
            (!state.freerdp.usable).then_some(Redetect),
            state.freerdp.error_code,
        ),
        diagnostic(
            SharedDirectory,
            if state.shared_directory_available {
                Success
            } else {
                Failure
            },
            None,
            (!state.shared_directory_available).then_some(OpenSettings),
            None,
        ),
        diagnostic(
            SharedMount,
            if state.shared_mount_matches {
                Success
            } else if state.compose_valid && state.shared_directory_available {
                Failure
            } else {
                Warning
            },
            None,
            (!state.shared_mount_matches).then_some(OpenSettings),
            None,
        ),
        diagnostic(
            Container,
            if state.container_status.is_running() {
                Success
            } else if matches!(
                state.container_status,
                ContainerStatus::Dead | ContainerStatus::Unknown
            ) {
                Failure
            } else {
                Warning
            },
            Some(container_status_name(state.container_status).to_string()),
            (!state.container_status.is_running()).then_some(StartWinboat),
            state.container_error_code,
        ),
        diagnostic(
            GuestApi,
            if state.guest_online {
                Success
            } else if state.container_status.is_running() {
                Failure
            } else {
                Warning
            },
            None,
            (!state.guest_online).then_some(if state.container_status.is_running() {
                OpenWinboat
            } else {
                StartWinboat
            }),
            state.guest_error_code,
        ),
        diagnostic(
            GuestClock,
            guest_clock_status(state),
            observed_guest_clock(state),
            guest_clock_action(state),
            guest_clock_error_code(state),
        ),
        diagnostic(
            Rdp,
            if state.rdp_reachable {
                Success
            } else if state.container_status.is_running() {
                Failure
            } else {
                Warning
            },
            None,
            (!state.rdp_reachable).then_some(if state.container_status.is_running() {
                OpenWinboat
            } else {
                StartWinboat
            }),
            state.rdp_error_code,
        ),
        diagnostic(
            MarketplaceBrowser,
            if state.browser.usable && state.browser_sandbox_available {
                Success
            } else if state.browser.usable {
                Failure
            } else {
                Warning
            },
            observed_browser(&state.browser, state.browser_sandbox_available),
            (!state.browser.usable || !state.browser_sandbox_available).then_some(Redetect),
            state.browser.error_code,
        ),
    ]
}

fn observed_tool(probe: &ToolProbe) -> Option<String> {
    probe
        .version
        .clone()
        .or_else(|| probe.available.then(|| "detected-but-unusable".to_string()))
}

fn guest_clock_status(state: &LinuxDiagnosticState) -> EnvironmentDiagnosticStatus {
    use EnvironmentDiagnosticStatus::{Failure, Success, Warning};

    if !state.guest_online {
        return if state.container_status.is_running() {
            Failure
        } else {
            Warning
        };
    }
    match state.guest_clock {
        None | Some(GuestClockProbe::Unmeasurable) => Warning,
        Some(GuestClockProbe::Measured(skew_seconds)) => {
            let magnitude = skew_seconds.unsigned_abs();
            if magnitude <= GUEST_CLOCK_SKEW_SUCCESS_SECONDS.unsigned_abs() {
                Success
            } else if magnitude <= GUEST_CLOCK_SKEW_WARNING_SECONDS.unsigned_abs() {
                Warning
            } else {
                Failure
            }
        }
    }
}

fn observed_guest_clock(state: &LinuxDiagnosticState) -> Option<String> {
    if !state.guest_online {
        return None;
    }
    let mut observed = match state.guest_clock.and_then(GuestClockProbe::skew_seconds) {
        Some(skew_seconds) => format!("skew={skew_seconds}s"),
        None => "skew=unavailable".to_string(),
    };
    if let Some(clocksource) = read_host_clocksource() {
        observed.push_str("; host-clocksource=");
        observed.push_str(&clocksource);
    }
    Some(observed)
}

fn guest_clock_action(state: &LinuxDiagnosticState) -> Option<EnvironmentDiagnosticAction> {
    use EnvironmentDiagnosticAction::{OpenWinboat, Redetect, StartWinboat};

    if !state.guest_online {
        return Some(if state.container_status.is_running() {
            OpenWinboat
        } else {
            StartWinboat
        });
    }
    (guest_clock_status(state) != EnvironmentDiagnosticStatus::Success).then_some(Redetect)
}

fn guest_clock_error_code(state: &LinuxDiagnosticState) -> Option<EnvironmentDiagnosticErrorCode> {
    (guest_clock_status(state) == EnvironmentDiagnosticStatus::Failure)
        .then_some(EnvironmentDiagnosticErrorCode::GuestClockSkewExceeded)
}

fn read_host_clocksource() -> Option<String> {
    let file = std::fs::File::open(HOST_CLOCKSOURCE_PATH).ok()?;
    let mut value = String::new();
    file.take(MAX_HOST_CLOCKSOURCE_BYTES)
        .read_to_string(&mut value)
        .ok()?;
    sanitized_host_clocksource(&value)
}

fn sanitized_host_clocksource(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 32
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        }))
    .then(|| value.to_string())
}

fn observed_browser(probe: &ToolProbe, sandbox_available: bool) -> Option<String> {
    if !probe.usable {
        return observed_tool(probe);
    }
    let sandbox = if sandbox_available {
        "sandbox=active"
    } else {
        "sandbox=unavailable"
    };
    probe
        .version
        .as_ref()
        .map(|version| format!("{version}; {sandbox}"))
        .or_else(|| probe.available.then(|| sandbox.to_string()))
}

fn diagnostic(
    id: EnvironmentDiagnosticId,
    status: EnvironmentDiagnosticStatus,
    observed: Option<String>,
    action: Option<EnvironmentDiagnosticAction>,
    error_code: Option<EnvironmentDiagnosticErrorCode>,
) -> EnvironmentDiagnostic {
    EnvironmentDiagnostic {
        id,
        status,
        observed,
        action,
        error_code,
    }
}

async fn probe_container_runtime(executable: &str) -> ToolProbe {
    if !path_exists_or_binary(executable) {
        return ToolProbe::default();
    }
    probe_tool(executable, &["info", "--format", "{{.ServerVersion}}"]).await
}

async fn probe_tool(executable: &str, arguments: &[&str]) -> ToolProbe {
    probe_tool_with_policy(executable, arguments, CommandPolicy::PROBE).await
}

async fn probe_tool_with_policy(
    executable: &str,
    arguments: &[&str],
    policy: CommandPolicy,
) -> ToolProbe {
    if !path_exists_or_binary(executable) {
        return ToolProbe::default();
    }
    let mut command = Command::new(executable);
    command.args(arguments);
    let output = match process::output(command, policy, None, "environment probe").await {
        Ok(output) => output,
        Err(error) => {
            return ToolProbe {
                available: true,
                error_code: Some(environment_error_code(error.kind())),
                ..ToolProbe::default()
            };
        }
    };
    let raw = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    ToolProbe {
        available: true,
        usable: output.status.success(),
        version: output
            .status
            .success()
            .then(|| extract_version(raw))
            .flatten(),
        error_code: None,
    }
}

const fn environment_error_code(kind: CommandFailureKind) -> EnvironmentDiagnosticErrorCode {
    match kind {
        CommandFailureKind::Spawn => EnvironmentDiagnosticErrorCode::ExternalProcessSpawnFailed,
        CommandFailureKind::Timeout => EnvironmentDiagnosticErrorCode::ExternalProcessTimeout,
        CommandFailureKind::Cancelled => EnvironmentDiagnosticErrorCode::ExternalProcessCancelled,
        CommandFailureKind::Wait | CommandFailureKind::Cleanup => {
            EnvironmentDiagnosticErrorCode::ExternalProcessInterrupted
        }
    }
}

fn extract_version(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '.' | '-' | '+' | '_')
            })
        })
        .find(|token| {
            token
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
                && token.contains('.')
                && token.len() <= 64
        })
        .map(ToString::to_string)
}

fn tcp_endpoint_reachable(host: &str, port: u16) -> bool {
    if !is_loopback_host(host) || port == 0 {
        return false;
    }
    (host, port)
        .to_socket_addrs()
        .ok()
        .into_iter()
        .flatten()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok())
}

const fn container_status_name(status: ContainerStatus) -> &'static str {
    match status {
        ContainerStatus::Created => "created",
        ContainerStatus::Restarting => "restarting",
        ContainerStatus::Running => "running",
        ContainerStatus::Removing => "removing",
        ContainerStatus::Paused => "paused",
        ContainerStatus::Exited => "exited",
        ContainerStatus::Dead => "dead",
        ContainerStatus::NotFound => "not-found",
        ContainerStatus::Unknown => "unknown",
    }
}

pub async fn start_container(config: &AppConfig) -> Result<ContainerStatus, String> {
    if !Path::new(&config.compose_file).is_file() {
        return Err(crate::tr!(
            "error-compose-file-not-found",
            path = &config.compose_file
        ));
    }
    let status = inspect_container_status(config).await;
    if status.is_running() {
        return Ok(status);
    }

    if status.exists() {
        let mut command = Command::new(config.container_runtime.as_str());
        command.arg("start").arg(&config.container_name);
        let output = process::output(command, lifecycle_policy(config), None, "container start")
            .await
            .map_err(|error| crate::tr!("error-container-start", error = error))?;
        ensure_success(output, &crate::tr!("operation-container-start"))?;
    } else {
        compose_up(config, false).await?;
    }
    Ok(inspect_container_status(config).await)
}

pub async fn recreate_container(config: &AppConfig) -> Result<(), String> {
    let service_name = winboat_compose_service_name(Path::new(&config.compose_file))?;
    recreate_compose_service(config, &service_name).await
}

pub async fn recreate_compose_service(
    config: &AppConfig,
    service_name: &str,
) -> Result<(), String> {
    compose_up_service(config, true, service_name).await
}

pub fn open_winboat(config: &AppConfig) -> Result<(), String> {
    let executable = resolved_winboat_executable(config)
        .ok_or_else(|| crate::tr!("error-winboat-executable-not-found"))?;
    StdCommand::new(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| crate::tr!("error-winboat-open", error = error))
}

async fn compose_up(config: &AppConfig, force_recreate: bool) -> Result<(), String> {
    let service_name = winboat_compose_service_name(Path::new(&config.compose_file))?;
    compose_up_service(config, force_recreate, &service_name).await
}

async fn compose_up_service(
    config: &AppConfig,
    force_recreate: bool,
    service_name: &str,
) -> Result<(), String> {
    compose_up_with_policy(
        config,
        force_recreate,
        service_name,
        config.container_runtime.as_str(),
        lifecycle_policy(config),
    )
    .await
}

pub(crate) async fn compose_up_with_policy(
    config: &AppConfig,
    force_recreate: bool,
    service_name: &str,
    runtime: &str,
    policy: CommandPolicy,
) -> Result<(), String> {
    let mut command;
    if config.container_runtime == ContainerRuntime::Podman
        && path_exists_or_binary("podman-compose")
    {
        command = Command::new("podman-compose");
    } else {
        command = Command::new(runtime);
        command.arg("compose");
    }
    command
        .arg("-f")
        .arg(&config.compose_file)
        .arg("up")
        .arg("-d");
    if force_recreate {
        command.arg("--force-recreate");
    }
    command.arg(service_name);
    let output = process::output(command, policy, None, "Compose apply")
        .await
        .map_err(|error| crate::tr!("error-compose-run", error = error))?;
    ensure_success(output, &crate::tr!("operation-compose-apply"))
}

fn lifecycle_policy(config: &AppConfig) -> CommandPolicy {
    CommandPolicy::new(
        Duration::from_secs(config.startup_timeout_seconds.clamp(1, 900)),
        256 * 1024,
    )
}

pub(super) async fn ensure_guest_online(config: &AppConfig) -> Result<(), String> {
    if guest_is_online(config).await {
        return Ok(());
    }
    start_container(config).await?;
    let timeout = Duration::from_secs(config.startup_timeout_seconds);
    let started = tokio::time::Instant::now();
    while started.elapsed() < timeout {
        if guest_is_online(config).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(crate::tr!(
        "error-guest-timeout",
        seconds = crate::i18n::format_number(config.startup_timeout_seconds)
    ))
}

pub(super) async fn ensure_private_operation_transport(config: &AppConfig) -> Result<(), String> {
    if !is_loopback_host(&config.rdp_host) {
        return Err(crate::tr!(
            "error-winboat-transport-public",
            endpoint = &config.rdp_host
        ));
    }
    let api_url = reqwest::Url::parse(&config.api_url)
        .map_err(|error| crate::tr!("error-winboat-transport-inspect", error = error))?;
    if !api_url.host_str().is_some_and(is_loopback_host) {
        return Err(crate::tr!(
            "error-winboat-transport-public",
            endpoint = api_url.as_str()
        ));
    }

    let inspection = runtime_inspection(config).await?;
    for guest_port in ["3389/tcp", "7148/tcp"] {
        let bindings = inspection
            .network_settings
            .ports
            .get(guest_port)
            .and_then(Option::as_ref)
            .filter(|bindings| !bindings.is_empty())
            .ok_or_else(|| {
                crate::tr!("error-winboat-transport-binding-missing", port = guest_port)
            })?;
        if bindings
            .iter()
            .any(|binding| !is_loopback_host(&binding.host_ip))
        {
            return Err(crate::tr!(
                "error-winboat-transport-public",
                endpoint = format!("{} ({guest_port})", bindings[0].host_ip)
            ));
        }
    }
    Ok(())
}

pub async fn guest_is_online(config: &AppConfig) -> bool {
    let api_url = runtime_host_port_async(config, 7148, "tcp")
        .await
        .ok()
        .flatten()
        .map(|port| format!("http://127.0.0.1:{port}"))
        .unwrap_or_else(|| config.api_url.clone());
    guest_is_online_at(&api_url).await
}

pub(super) async fn guest_is_online_at(api_url: &str) -> bool {
    guest_api_probe(api_url).await.online
}

async fn guest_api_probe(api_url: &str) -> GuestApiProbe {
    let Ok(client) = http_client(Duration::from_secs(2)) else {
        return GuestApiProbe::default();
    };
    let requested_at = Utc::now();
    let Ok(response) = client.get(format!("{api_url}/health")).send().await else {
        return GuestApiProbe::default();
    };
    let responded_at = Utc::now();
    if !response.status().is_success() {
        return GuestApiProbe::default();
    }
    GuestApiProbe {
        online: true,
        clock: Some(guest_clock_from_response(
            response.headers().get(reqwest::header::DATE),
            requested_at,
            responded_at,
        )),
    }
}

/// Derives guest-minus-host clock skew from the Guest API HTTP `Date` header.
/// The Go guest server generates the header from the Windows guest system
/// clock, while the host midpoint bounds the measurement by request latency.
fn guest_clock_from_response(
    date_header: Option<&reqwest::header::HeaderValue>,
    requested_at: DateTime<Utc>,
    responded_at: DateTime<Utc>,
) -> GuestClockProbe {
    let Some(date) = date_header
        .and_then(|value| value.to_str().ok())
        .and_then(parse_http_date)
    else {
        return GuestClockProbe::Unmeasurable;
    };
    let elapsed_milliseconds = (responded_at - requested_at).num_milliseconds();
    let host_midpoint = requested_at + chrono::TimeDelta::milliseconds(elapsed_milliseconds / 2);
    GuestClockProbe::Measured((date - host_midpoint).num_seconds())
}

fn parse_http_date(value: &str) -> Option<DateTime<Utc>> {
    let normalized = value.trim().replace(" GMT", " +0000");
    DateTime::parse_from_rfc2822(&normalized)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

pub(crate) async fn guest_is_online_at_url(api_url: &str) -> bool {
    guest_is_online_at(api_url).await
}

pub(super) fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("mendimaru/0.1 (WinBoat Studio Pro manager)")
        .build()
        .map_err(|error| crate::tr!("error-http-client-create", error = error))
}

pub(crate) async fn inspect_container_status(config: &AppConfig) -> ContainerStatus {
    inspect_container(config).await.status
}

#[derive(Debug)]
struct ContainerInspection {
    status: ContainerStatus,
    shared_directory: Option<String>,
    error_code: Option<EnvironmentDiagnosticErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeHostBinding {
    pub(crate) host_ip: String,
    pub(crate) host_port: u16,
    pub(crate) guest_port: u16,
}

impl ContainerInspection {
    fn not_found() -> Self {
        Self {
            status: ContainerStatus::NotFound,
            shared_directory: None,
            error_code: None,
        }
    }

    fn unknown() -> Self {
        Self {
            status: ContainerStatus::Unknown,
            shared_directory: None,
            error_code: None,
        }
    }

    fn failed(error: &CommandFailure) -> Self {
        Self {
            status: ContainerStatus::Unknown,
            shared_directory: None,
            error_code: Some(environment_error_code(error.kind())),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeContainerInspection {
    state: RuntimeContainerState,
    #[serde(default)]
    mounts: Vec<RuntimeContainerMount>,
    #[serde(default)]
    network_settings: RuntimeNetworkSettings,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeNetworkSettings {
    #[serde(default)]
    ports: BTreeMap<String, Option<Vec<RuntimePortBinding>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimePortBinding {
    host_ip: String,
    host_port: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeContainerState {
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RuntimeContainerMount {
    source: String,
    destination: String,
}

async fn inspect_container(config: &AppConfig) -> ContainerInspection {
    let mut command = Command::new(config.container_runtime.as_str());
    command.arg("inspect").arg(&config.container_name);
    let output =
        process::output(command, CommandPolicy::STATUS, None, "container inspection").await;
    match output {
        Ok(output) if output.status.success() => {
            parse_container_inspection(&output.stdout).unwrap_or_else(ContainerInspection::unknown)
        }
        Ok(_) => ContainerInspection::not_found(),
        Err(error) => ContainerInspection::failed(&error),
    }
}

async fn runtime_inspection(config: &AppConfig) -> Result<RuntimeContainerInspection, String> {
    let mut command = Command::new(config.container_runtime.as_str());
    command.arg("inspect").arg(&config.container_name);
    let output = process::output(command, CommandPolicy::STATUS, None, "container inspection")
        .await
        .map_err(|error| crate::tr!("error-winboat-transport-inspect", error = error))?;
    if !output.status.success() {
        return Err(crate::tr!(
            "error-winboat-transport-inspect",
            error = output.status
        ));
    }
    serde_json::from_slice::<Vec<RuntimeContainerInspection>>(&output.stdout)
        .map_err(|error| crate::tr!("error-winboat-transport-inspect", error = error))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            crate::tr!(
                "error-winboat-transport-inspect",
                error = "the runtime returned no container"
            )
        })
}

pub(crate) async fn runtime_host_binding(
    config: &AppConfig,
    guest_port: u16,
) -> Result<RuntimeHostBinding, String> {
    let inspection = runtime_inspection(config).await?;
    if !ContainerStatus::from_runtime(&inspection.state.status).is_running() {
        return Err("the WinBoat container is not running".to_string());
    }
    let private_port = format!("{guest_port}/tcp");
    let bindings = inspection
        .network_settings
        .ports
        .get(&private_port)
        .and_then(Option::as_ref)
        .filter(|bindings| !bindings.is_empty())
        .ok_or_else(|| format!("the WinBoat container has no mapping for {private_port}"))?;
    if bindings.len() != 1 {
        return Err(format!(
            "the WinBoat container has multiple mappings for {private_port}"
        ));
    }
    let binding = &bindings[0];
    if !is_loopback_host(&binding.host_ip) {
        return Err(format!(
            "the WinBoat Runtime mapping for {private_port} is not loopback-only"
        ));
    }
    let host_port = binding
        .host_port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| {
            format!("the WinBoat Runtime mapping for {private_port} has no host port")
        })?;
    Ok(RuntimeHostBinding {
        host_ip: binding.host_ip.clone(),
        host_port,
        guest_port,
    })
}

pub(crate) async fn storage_mount_identity(config: &AppConfig) -> Result<Vec<String>, String> {
    let inspection = runtime_inspection(config).await?;
    let mut sources = inspection
        .mounts
        .into_iter()
        .filter(|mount| mount.destination.eq_ignore_ascii_case("/storage"))
        .map(|mount| mount.source)
        .collect::<Vec<_>>();
    sources.sort();
    sources.dedup();
    if sources.is_empty() {
        return Err("the WinBoat container has no /storage mount".to_string());
    }
    Ok(sources)
}

fn parse_container_inspection(output: &[u8]) -> Option<ContainerInspection> {
    let containers = serde_json::from_slice::<Vec<RuntimeContainerInspection>>(output).ok()?;
    let container = containers.into_iter().next()?;
    let shared_directory = container
        .mounts
        .into_iter()
        .find(|mount| mount.destination == "/shared")
        .map(|mount| mount.source);
    Some(ContainerInspection {
        status: ContainerStatus::from_runtime(&container.state.status),
        shared_directory,
        error_code: None,
    })
}

fn ensure_success(output: CommandOutput, operation: &str) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let mut stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let captured_truncated = output.stdout_truncated || output.stderr_truncated;
    if captured_truncated {
        stderr.push_str(" [output truncated]");
    }
    if stderr.is_empty() {
        Err(crate::tr!(
            "error-operation-status",
            operation = operation,
            status = output.status
        ))
    } else {
        Err(crate::tr!(
            "error-operation-detail",
            operation = operation,
            detail = stderr
        ))
    }
}

fn paths_refer_to_same_location(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left_path == right_path,
    }
}

fn is_loopback_host(value: &str) -> bool {
    value.eq_ignore_ascii_case("localhost")
        || value
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::{
        build_linux_diagnostics, compose_up_with_policy, environment_status, extract_version,
        guest_clock_from_response, is_loopback_host, parse_container_inspection, parse_http_date,
        probe_tool_with_policy, sanitized_host_clocksource, GuestClockProbe, LinuxDiagnosticState,
        RuntimeContainerInspection, ToolProbe,
    };
    use crate::models::{
        AppConfig, ContainerRuntime, ContainerStatus, EnvironmentDiagnosticAction,
        EnvironmentDiagnosticErrorCode, EnvironmentDiagnosticId, EnvironmentDiagnosticStatus,
    };
    use crate::process::CommandPolicy;
    use chrono::{DateTime, Utc};
    use std::time::Duration;

    #[test]
    fn parses_status_and_active_shared_mount_from_runtime_inspection() {
        let inspection = parse_container_inspection(
            br#"[{"State":{"Status":"running"},"Mounts":[{"Source":"/home/dev/Mendix","Destination":"/shared"},{"Source":"volume","Destination":"/storage"}]}]"#,
        )
        .expect("runtime inspection parses");

        assert_eq!(inspection.status, ContainerStatus::Running);
        assert_eq!(
            inspection.shared_directory.as_deref(),
            Some("/home/dev/Mendix")
        );
    }

    #[test]
    fn accepts_only_explicit_loopback_transport_bindings() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.50"));

        let inspection = serde_json::from_str::<Vec<RuntimeContainerInspection>>(
            r#"[{"State":{"Status":"running"},"Mounts":[],"NetworkSettings":{"Ports":{"3389/tcp":[{"HostIp":"127.0.0.1","HostPort":"47300"}],"7148/tcp":[{"HostIp":"::1","HostPort":"47280"}]}}}]"#,
        )
        .expect("port bindings parse");
        assert_eq!(inspection[0].network_settings.ports.len(), 2);
    }

    fn healthy_state() -> LinuxDiagnosticState {
        LinuxDiagnosticState {
            winboat_available: true,
            compose_available: true,
            compose_valid: true,
            runtime: ToolProbe {
                available: true,
                usable: true,
                version: Some("29.7.2".to_string()),
                error_code: None,
            },
            freerdp: ToolProbe {
                available: true,
                usable: true,
                version: Some("3.30.0".to_string()),
                error_code: None,
            },
            shared_directory_available: true,
            shared_mount_matches: true,
            container_status: ContainerStatus::Running,
            container_error_code: None,
            guest_online: true,
            guest_error_code: None,
            guest_clock: Some(GuestClockProbe::Measured(0)),
            rdp_reachable: true,
            rdp_error_code: None,
            browser: ToolProbe {
                available: true,
                usable: true,
                version: Some("151.0.7922.137".to_string()),
                error_code: None,
            },
            browser_sandbox_available: true,
        }
    }

    #[test]
    fn healthy_environment_has_independent_successful_checks() {
        let state = healthy_state();
        let diagnostics = build_linux_diagnostics(&state);

        assert!(state.ready());
        assert_eq!(diagnostics.len(), 11);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.status == EnvironmentDiagnosticStatus::Success));
        let guest_clock = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestClock)
            .expect("guest clock diagnostic");
        assert_eq!(guest_clock.error_code, None);
        assert_eq!(
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::ContainerRuntime)
                .and_then(|diagnostic| diagnostic.observed.as_deref()),
            Some("29.7.2")
        );
        assert_eq!(
            diagnostics
                .iter()
                .find(|diagnostic| { diagnostic.id == EnvironmentDiagnosticId::MarketplaceBrowser })
                .and_then(|diagnostic| diagnostic.observed.as_deref()),
            Some("151.0.7922.137; sandbox=active")
        );
    }

    #[test]
    fn partial_failures_keep_recovery_actions_and_unrelated_successes() {
        let mut state = healthy_state();
        state.compose_valid = false;
        state.guest_online = false;
        state.browser = ToolProbe::default();
        state.browser_sandbox_available = false;
        let diagnostics = build_linux_diagnostics(&state);

        assert!(!state.ready());
        let compose = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::Compose)
            .expect("compose check");
        assert_eq!(compose.status, EnvironmentDiagnosticStatus::Failure);
        assert_eq!(
            compose.action,
            Some(EnvironmentDiagnosticAction::OpenSettings)
        );
        let guest = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestApi)
            .expect("guest check");
        assert_eq!(guest.status, EnvironmentDiagnosticStatus::Failure);
        assert_eq!(guest.action, Some(EnvironmentDiagnosticAction::OpenWinboat));
        let browser = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::MarketplaceBrowser)
            .expect("browser check");
        assert_eq!(browser.status, EnvironmentDiagnosticStatus::Warning);
        assert_eq!(browser.action, Some(EnvironmentDiagnosticAction::Redetect));
        assert_eq!(
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::Freerdp)
                .map(|diagnostic| diagnostic.status),
            Some(EnvironmentDiagnosticStatus::Success)
        );
    }

    #[test]
    fn guest_clock_skew_uses_bounded_thresholds() {
        for (skew_seconds, expected) in [
            (0, EnvironmentDiagnosticStatus::Success),
            (5, EnvironmentDiagnosticStatus::Success),
            (-5, EnvironmentDiagnosticStatus::Success),
            (6, EnvironmentDiagnosticStatus::Warning),
            (-6, EnvironmentDiagnosticStatus::Warning),
            (60, EnvironmentDiagnosticStatus::Warning),
            (61, EnvironmentDiagnosticStatus::Failure),
            (-36_000, EnvironmentDiagnosticStatus::Failure),
        ] {
            let mut state = healthy_state();
            state.guest_clock = Some(GuestClockProbe::Measured(skew_seconds));
            let diagnostics = build_linux_diagnostics(&state);
            let guest_clock = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestClock)
                .expect("guest clock diagnostic");
            assert_eq!(guest_clock.status, expected, "skew {skew_seconds}s");
            assert_eq!(
                guest_clock.error_code,
                (expected == EnvironmentDiagnosticStatus::Failure)
                    .then_some(EnvironmentDiagnosticErrorCode::GuestClockSkewExceeded),
                "skew {skew_seconds}s"
            );
        }
    }

    #[test]
    fn guest_clock_unmeasurable_and_offline_paths_have_recovery_actions() {
        let mut unmeasurable = healthy_state();
        unmeasurable.guest_clock = Some(GuestClockProbe::Unmeasurable);
        let diagnostic = build_linux_diagnostics(&unmeasurable)
            .into_iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestClock)
            .expect("guest clock diagnostic");
        assert_eq!(diagnostic.status, EnvironmentDiagnosticStatus::Warning);
        assert_eq!(
            diagnostic.action,
            Some(EnvironmentDiagnosticAction::Redetect)
        );
        assert_eq!(diagnostic.error_code, None);
        assert!(
            diagnostic
                .observed
                .as_deref()
                .is_some_and(|observed| observed.starts_with("skew=unavailable")),
            "observed should describe the unavailable measurement: {:?}",
            diagnostic.observed
        );

        let mut offline = healthy_state();
        offline.guest_online = false;
        offline.guest_clock = None;
        let diagnostic = build_linux_diagnostics(&offline)
            .into_iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestClock)
            .expect("guest clock diagnostic");
        assert_eq!(diagnostic.status, EnvironmentDiagnosticStatus::Failure);
        assert_eq!(
            diagnostic.action,
            Some(EnvironmentDiagnosticAction::OpenWinboat)
        );
        assert_eq!(diagnostic.observed, None);
    }

    #[test]
    fn guest_clock_observes_measured_skew_and_a_safe_host_clocksource() {
        let mut state = healthy_state();
        state.guest_clock = Some(GuestClockProbe::Measured(-36_000));
        let diagnostic = build_linux_diagnostics(&state)
            .into_iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::GuestClock)
            .expect("guest clock diagnostic");
        let observed = diagnostic
            .observed
            .as_deref()
            .expect("measured skew is observable");
        assert!(
            observed.starts_with("skew=-36000s"),
            "unexpected observed value: {observed}"
        );
        if let Some((_, clocksource)) = observed.rsplit_once("; host-clocksource=") {
            assert!(
                sanitized_host_clocksource(clocksource).is_some(),
                "host clocksource must remain sanitized: {clocksource}"
            );
        }
    }

    #[test]
    fn host_clocksource_accepts_only_bounded_safe_names() {
        assert_eq!(sanitized_host_clocksource("tsc\n"), Some("tsc".to_string()));
        assert_eq!(
            sanitized_host_clocksource("kvm-clock"),
            Some("kvm-clock".to_string())
        );
        assert_eq!(sanitized_host_clocksource(""), None);
        assert_eq!(sanitized_host_clocksource("hpet; rm -rf /"), None);
        assert_eq!(sanitized_host_clocksource(&"x".repeat(33)), None);
        assert_eq!(sanitized_host_clocksource("UPPER"), None);
    }

    #[test]
    fn parses_http_dates_from_the_guest_server() {
        let parsed = parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").expect("HTTP date parses");
        assert_eq!(parsed.to_rfc3339(), "1994-11-06T08:49:37+00:00");
        assert!(parse_http_date("not-a-date").is_none());
        assert!(parse_http_date("").is_none());
    }

    #[test]
    fn guest_clock_skew_uses_the_host_midpoint() {
        let requested_at = DateTime::parse_from_rfc3339("2026-09-01T04:48:18Z")
            .expect("request time")
            .with_timezone(&Utc);
        let responded_at = requested_at + chrono::TimeDelta::milliseconds(200);
        let header = reqwest::header::HeaderValue::from_str("Sun, 06 Nov 1994 08:49:37 GMT")
            .expect("date header");

        let guest_ahead = guest_clock_from_response(Some(&header), requested_at, responded_at);
        assert_eq!(
            guest_ahead,
            GuestClockProbe::Measured(
                (parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").expect("guest time")
                    - (requested_at + chrono::TimeDelta::milliseconds(100)))
                .num_seconds()
            )
        );

        let missing = guest_clock_from_response(None, requested_at, responded_at);
        assert_eq!(missing, GuestClockProbe::Unmeasurable);

        let invalid =
            reqwest::header::HeaderValue::from_str("invalid").expect("invalid header value");
        assert_eq!(
            guest_clock_from_response(Some(&invalid), requested_at, responded_at),
            GuestClockProbe::Unmeasurable
        );
    }

    #[test]
    fn usable_browser_without_a_verified_sandbox_is_a_failure() {
        let mut state = healthy_state();
        state.browser_sandbox_available = false;
        let diagnostics = build_linux_diagnostics(&state);
        let browser = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::MarketplaceBrowser)
            .expect("browser check");

        assert_eq!(browser.status, EnvironmentDiagnosticStatus::Failure);
        assert_eq!(browser.action, Some(EnvironmentDiagnosticAction::Redetect));
        assert_eq!(
            browser.observed.as_deref(),
            Some("151.0.7922.137; sandbox=unavailable")
        );
    }

    #[test]
    fn version_output_is_reduced_to_a_safe_component_version() {
        assert_eq!(
            extract_version(b"Google Chrome 151.0.7922.137\n"),
            Some("151.0.7922.137".to_string())
        );
        assert_eq!(
            extract_version(b"This is FreeRDP version 3.30.0 (build)"),
            Some("3.30.0".to_string())
        );
        assert_eq!(extract_version(b"password=secret"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_probe_has_a_stable_diagnostic_error_code() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().expect("temporary directory");
        let executable = temporary.path().join("fake-docker");
        std::fs::write(
            &executable,
            "#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n",
        )
        .expect("write probe fixture");
        let mut permissions = std::fs::metadata(&executable)
            .expect("probe fixture metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).expect("make fixture executable");

        let probe = probe_tool_with_policy(
            executable.to_str().expect("fixture path"),
            &["info"],
            CommandPolicy::new(Duration::from_millis(100), 1024),
        )
        .await;

        assert!(probe.available);
        assert!(!probe.usable);
        assert_eq!(
            probe.error_code,
            Some(EnvironmentDiagnosticErrorCode::ExternalProcessTimeout)
        );
        let mut state = healthy_state();
        state.runtime = probe;
        let diagnostic = build_linux_diagnostics(&state)
            .into_iter()
            .find(|diagnostic| diagnostic.id == EnvironmentDiagnosticId::ContainerRuntime)
            .expect("runtime diagnostic");
        assert_eq!(
            diagnostic.error_code,
            Some(EnvironmentDiagnosticErrorCode::ExternalProcessTimeout)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn force_recreate_command_targets_only_the_identified_service() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let runtime = temporary.path().join("fake-docker");
        let captured = temporary.path().join("arguments");
        let compose = temporary.path().join("compose.yml");
        std::fs::write(&compose, "services: {}\n").expect("Compose fixture");
        std::fs::write(
            &runtime,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                captured.display()
            ),
        )
        .expect("runtime fixture");
        let mut permissions = std::fs::metadata(&runtime)
            .expect("runtime metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&runtime, permissions).expect("executable runtime");
        let config = AppConfig {
            language_preference: "system".into(),
            winboat_setup_pending: false,
            winboat_executable: "winboat".into(),
            compose_file: compose.to_string_lossy().to_string(),
            container_runtime: ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:47280".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 47300,
            shared_directory: temporary.path().to_string_lossy().to_string(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "xfreerdp3".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 180,
        };

        compose_up_with_policy(
            &config,
            true,
            "winboat-vm",
            runtime.to_str().expect("runtime path"),
            CommandPolicy::new(Duration::from_secs(1), 1024),
        )
        .await
        .expect("Compose command succeeds");

        let arguments = std::fs::read_to_string(captured).expect("captured arguments");
        assert_eq!(
            arguments.lines().collect::<Vec<_>>(),
            [
                "compose",
                "-f",
                compose.to_str().expect("Compose path"),
                "up",
                "-d",
                "--force-recreate",
                "winboat-vm",
            ]
        );
    }

    #[test]
    #[ignore = "checks the real Linux host, WinBoat guest, RDP endpoint, and Marketplace browser"]
    fn live_e2e_reports_every_linux_dependency_without_secrets() {
        use crate::models::{environment_diagnostic_report, EnvironmentDiagnosticId};
        use std::collections::BTreeSet;

        crate::i18n::initialize("en-US").expect("English localization initializes");
        let config = crate::config::detect_config().expect("live configuration is detected");
        let status = tauri::async_runtime::block_on(environment_status(&config));
        assert!(status.ready, "live environment diagnostics: {status:#?}");
        assert_eq!(status.diagnostics.len(), 11);
        let ids = status
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), status.diagnostics.len());
        for expected in [
            EnvironmentDiagnosticId::Winboat,
            EnvironmentDiagnosticId::Compose,
            EnvironmentDiagnosticId::ContainerRuntime,
            EnvironmentDiagnosticId::Freerdp,
            EnvironmentDiagnosticId::SharedDirectory,
            EnvironmentDiagnosticId::SharedMount,
            EnvironmentDiagnosticId::Container,
            EnvironmentDiagnosticId::GuestApi,
            EnvironmentDiagnosticId::GuestClock,
            EnvironmentDiagnosticId::Rdp,
            EnvironmentDiagnosticId::MarketplaceBrowser,
        ] {
            assert!(ids.contains(&expected), "missing {expected:?}");
        }
        assert!(status.diagnostics.iter().all(|diagnostic| {
            diagnostic.status == EnvironmentDiagnosticStatus::Success
                || (diagnostic.id == EnvironmentDiagnosticId::GuestClock
                    && diagnostic.status == EnvironmentDiagnosticStatus::Warning)
        }));

        let report = environment_diagnostic_report(&status).expect("report serializes");
        for sensitive in [
            &config.shared_directory,
            &config.compose_file,
            &config.winboat_executable,
            &config.windows_shared_directory,
        ] {
            assert!(
                sensitive.is_empty() || !report.contains(sensitive),
                "report leaked configured path"
            );
        }
        assert!(!report.to_ascii_lowercase().contains("password"));
        assert!(!report.to_ascii_lowercase().contains("token"));
    }
}
