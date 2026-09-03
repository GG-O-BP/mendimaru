use crate::app_paths::AppPaths;
use crate::contracts::{
    BackendError, BackendErrorCode, BackendId, BrowserTestPolicy, CapabilityId, CapabilitySnapshot,
    PlatformId, RuntimeMode, SessionDescriptor, CONTRACT_SCHEMA_VERSION,
};
use crate::models::{CommandError, CommandErrorCode, DownloadProgress};
#[cfg(target_os = "linux")]
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::Write;
use std::str::FromStr;
use std::time::Duration;

const EXIT_OK: i32 = 0;
const EXIT_OPERATION_FAILED: i32 = 1;
const EXIT_INVALID_REQUEST: i32 = 2;
const EXIT_BACKEND_UNAVAILABLE: i32 = 3;
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
const MAX_TIMEOUT_SECONDS: u64 = 3_600;
const DEFAULT_BROWSER_NAVIGATION_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_BROWSER_ACTION_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_BROWSER_ASSERTION_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_BROWSER_ARTIFACT_MIB: u64 = 128;
const DEFAULT_BROWSER_RETENTION_RUNS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Ndjson,
}

#[derive(Debug, PartialEq, Eq)]
enum CliCommand {
    Capabilities,
    EnvironmentStatus,
    EnvironmentEnsure,
    StudioList,
    StudioInstall {
        version: String,
        force_redownload: bool,
    },
    StudioUninstall {
        version: String,
    },
    StudioStart {
        version: String,
        project_id: Option<String>,
    },
    StudioStatus {
        session_id: Option<String>,
        refresh: bool,
        orphan_filter: bool,
    },
    StudioStop {
        session_id: String,
    },
    RuntimeBuild {
        project_id: String,
        clean: bool,
    },
    RuntimeStart {
        project_id: Option<String>,
        clean: bool,
        mode: RuntimeMode,
        studio_session_id: Option<String>,
        guest_port: Option<u16>,
    },
    RuntimeList,
    RuntimeStatus {
        session_id: String,
    },
    RuntimeWait {
        session_id: String,
    },
    RuntimeUrl {
        session_id: String,
    },
    RuntimeStop {
        session_id: String,
    },
    RuntimeForget {
        session_id: String,
    },
    RuntimeLogs {
        session_id: String,
        cursor: Option<String>,
    },
    BrowserDoctor,
    BrowserInstallChromium,
    BrowserTest {
        base_url: Option<String>,
        runtime_session_id: Option<String>,
        suite_path: String,
        policy: BrowserTestPolicy,
    },
    BrowserArtifacts {
        session_id: String,
    },
    ProjectList,
    ProjectVersion {
        project_id: String,
    },
    OperationList,
    OperationStatus {
        operation_id: String,
    },
    OperationRetry {
        operation_id: String,
    },
}

impl CliCommand {
    fn name(&self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::EnvironmentStatus => "env.status",
            Self::EnvironmentEnsure => "env.ensure",
            Self::StudioList => "studio.list",
            Self::StudioInstall { .. } => "studio.install",
            Self::StudioUninstall { .. } => "studio.uninstall",
            Self::StudioStart { .. } => "studio.start",
            Self::StudioStatus { .. } => "studio.status",
            Self::StudioStop { .. } => "studio.stop",
            Self::RuntimeBuild { .. } => "runtime.build",
            Self::RuntimeStart { .. } => "runtime.start",
            Self::RuntimeList => "runtime.list",
            Self::RuntimeStatus { .. } => "runtime.status",
            Self::RuntimeWait { .. } => "runtime.wait",
            Self::RuntimeUrl { .. } => "runtime.url",
            Self::RuntimeStop { .. } => "runtime.stop",
            Self::RuntimeForget { .. } => "runtime.forget",
            Self::RuntimeLogs { .. } => "runtime.logs",
            Self::BrowserDoctor => "browser.doctor",
            Self::BrowserInstallChromium => "browser.install",
            Self::BrowserTest { .. } => "browser.test",
            Self::BrowserArtifacts { .. } => "browser.artifacts",
            Self::ProjectList => "project.list",
            Self::ProjectVersion { .. } => "project.version",
            Self::OperationList => "operation.list",
            Self::OperationStatus { .. } => "operation.status",
            Self::OperationRetry { .. } => "operation.retry",
        }
    }
}

#[derive(Debug)]
struct ParsedCli {
    command: CliCommand,
    backend: Option<BackendId>,
    format: OutputFormat,
    timeout: Duration,
    include_snapshot: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SuccessEnvelope<'a> {
    schema_version: &'static str,
    command: &'a str,
    ok: bool,
    platform: PlatformId,
    backend: BackendId,
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    capability_snapshot: Option<&'a CapabilitySnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    studio_session_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_session_id: Option<&'a str>,
    data: &'a Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope<'a> {
    schema_version: &'static str,
    command: &'a str,
    ok: bool,
    platform: PlatformId,
    backend: BackendId,
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    capability_snapshot: Option<&'a CapabilitySnapshot>,
    error: &'a BackendError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEnvelope<'a> {
    schema_version: &'static str,
    command: &'a str,
    event: &'static str,
    session_id: &'a str,
    progress: &'a DownloadProgress,
}

#[derive(Debug)]
struct CommandOutput {
    data: Value,
    operation_id: Option<String>,
    studio_session_id: Option<String>,
    runtime_session_id: Option<String>,
    progress: Vec<DownloadProgress>,
}

impl CommandOutput {
    fn data(data: impl Serialize) -> Result<Self, CommandError> {
        Ok(Self {
            data: serde_json::to_value(data).map_err(|_| {
                CommandError::new(
                    CommandErrorCode::OperationFailed,
                    "the command result could not be serialized".to_string(),
                )
            })?,
            operation_id: None,
            studio_session_id: None,
            runtime_session_id: None,
            progress: Vec::new(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CliExecution {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

struct ExecutionContext {
    snapshot: CapabilitySnapshot,
    session: SessionDescriptor,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionKeeperResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    studio_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BackendError>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionKeeperIpcResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<crate::contracts::StudioSessionStatus>,
}

/// Runs a recognized headless command before Tauri is initialized. Returning
/// `None` means that the process should continue as the desktop application.
pub fn dispatch_from_env() -> Option<i32> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.is_empty() {
        return None;
    }
    if matches!(
        arguments.first().and_then(|value| value.to_str()),
        Some("--help" | "-h")
    ) {
        return Some(write_cli_execution(root_help_execution()));
    }
    if arguments.first().and_then(|value| value.to_str()) == Some("__runtime-supervisor") {
        return Some(crate::portable_runtime::supervisor_dispatch(
            &arguments[1..],
        ));
    }
    #[cfg(target_os = "linux")]
    if arguments.first().and_then(|value| value.to_str()) == Some("__session-keeper") {
        return Some(session_keeper_dispatch(&arguments[1..]));
    }
    let execution = execute(&arguments)?;
    if !execution.stdout.is_empty() {
        let _ = std::io::stdout().write_all(execution.stdout.as_bytes());
    }
    if !execution.stderr.is_empty() {
        let _ = std::io::stderr().write_all(execution.stderr.as_bytes());
    }
    Some(execution.exit_code)
}

fn write_cli_execution(execution: CliExecution) -> i32 {
    if !execution.stdout.is_empty() {
        let _ = std::io::stdout().write_all(execution.stdout.as_bytes());
    }
    if !execution.stderr.is_empty() {
        let _ = std::io::stderr().write_all(execution.stderr.as_bytes());
    }
    execution.exit_code
}

fn execute(arguments: &[OsString]) -> Option<CliExecution> {
    if arguments.is_empty() {
        return None;
    }
    if let Some(execution) = help_execution(arguments) {
        return Some(execution);
    }
    if !is_headless_command(arguments.first()) {
        return Some(unknown_command_execution(arguments));
    }
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(error) => return Some(bootstrap_error_execution(arguments, error)),
    };
    let command_name = parsed.command.name();
    if crate::i18n::initialize("en-US").is_err() {
        return Some(bootstrap_error_execution(
            arguments,
            BackendError::operation(
                expected_backend(),
                CapabilityId::StudioDetect,
                "localization initialization failed",
            ),
        ));
    }

    let context = match execution_context(parsed.backend) {
        Ok(context) => context,
        Err(error) => return Some(context_error_execution(command_name, parsed.format, error)),
    };
    let future = run_command(&parsed.command, parsed.timeout, &context.snapshot);
    let result = tauri::async_runtime::block_on(async {
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => result,
            _ = tokio::time::sleep(parsed.timeout) => Err(CommandError::new(
                CommandErrorCode::OperationFailed,
                "the command reached its timeout and was cancelled".to_string(),
            )),
            signal = tokio::signal::ctrl_c() => {
                let message = if signal.is_ok() {
                    "the command was cancelled"
                } else {
                    "the cancellation signal handler failed"
                };
                Err(CommandError::new(CommandErrorCode::OperationFailed, message.to_string()))
            }
        }
    });
    let include_snapshot =
        parsed.include_snapshot || matches!(parsed.command, CliCommand::Capabilities);
    Some(match result {
        Ok(output) => success_execution(
            command_name,
            parsed.format,
            &context,
            output,
            include_snapshot,
        ),
        Err(error) => error_execution(
            command_name,
            parsed.format,
            &context,
            command_error_to_backend(error, context.snapshot.manifest.backend),
            include_snapshot,
        ),
    })
}

fn help_execution(arguments: &[OsString]) -> Option<CliExecution> {
    let mut values = arguments.iter().filter_map(|value| value.to_str());
    let mut command_values = Vec::new();
    let mut help_requested = false;
    let mut option_value_expected = false;
    for value in values.by_ref() {
        if option_value_expected {
            option_value_expected = false;
            continue;
        }
        match value {
            "--help" | "-h" => {
                help_requested = true;
                break;
            }
            "--json" | "--ndjson" | "--snapshot" => {}
            "--backend" | "--timeout-seconds" => option_value_expected = true,
            value if value.starts_with("--backend=") || value.starts_with("--timeout-seconds=") => {
            }
            value => command_values.push(value),
        }
    }
    if !help_requested {
        return None;
    }
    if command_values.is_empty() {
        return Some(root_help_execution());
    }
    let help = subcommand_help(&command_values)?;
    Some(CliExecution {
        exit_code: EXIT_OK,
        stdout: format!("{help}\n"),
        stderr: String::new(),
    })
}

fn subcommand_help(values: &[&str]) -> Option<&'static str> {
    let subcommand = values.get(1).copied();
    match (values.first().copied(), subcommand) {
        (Some("capabilities"), None) => Some(
            "Usage: mendimaru capabilities [--backend BACKEND]\n\
             \n\
             Prints the immutable backend capability snapshot. Global --json,\n\
             --ndjson, --backend, and --timeout-seconds options are accepted.",
        ),
        (Some("env"), None) => Some(
            "Usage: mendimaru env COMMAND\n\
             \n\
             Commands:\n\
               status   Report dependency, container, guest, and shared-directory status\n\
               ensure   Ensure required environment dependencies are ready",
        ),
        (Some("env"), Some("status")) => Some(
            "Usage: mendimaru env status\n\
             \n\
             Reports the current environment status without mutating it.",
        ),
        (Some("env"), Some("ensure")) => Some(
            "Usage: mendimaru env ensure\n\
             \n\
             Ensures required environment dependencies are ready. Use\n\
             --timeout-seconds to bound the operation.",
        ),
        (Some("studio"), None) => Some(
            "Usage: mendimaru studio COMMAND\n\
             \n\
             Commands: list, install, uninstall, start, status, stop",
        ),
        (Some("studio"), Some("list")) => Some(
            "Usage: mendimaru studio list\n\
             \n\
             Lists authoritative installed Studio Pro versions.",
        ),
        (Some("studio"), Some("install")) => Some(
            "Usage: mendimaru studio install --version VERSION [--force-redownload]\n\
             \n\
             Installs an exact Studio Pro version. --force-redownload discards a\n\
             matching cached installer; --ndjson emits download progress.",
        ),
        (Some("studio"), Some("uninstall")) => Some(
            "Usage: mendimaru studio uninstall --version VERSION\n\
             \n\
             Uninstalls one exact Studio Pro version.",
        ),
        (Some("studio"), Some("start")) => Some(
            "Usage: mendimaru studio start --version VERSION [--project-id PROJECT_ID]\n\
             \n\
             Starts Studio Pro. A project ID requires the exact declared version.",
        ),
        (Some("studio"), Some("status")) => Some(
            "Usage: mendimaru studio status [--session-id STUDIO_SESSION_ID] [--refresh] [--orphans]\n\
             \n\
             Without --session-id, reports all Studio sessions. With it, reports\n\
             one selected session. The default Linux summary checks the trusted\n\
             session keeper; --refresh queries the authoritative WinBoat guest;\n\
             --orphans returns only sessions not owned by a keeper.",
        ),
        (Some("studio"), Some("stop")) => Some(
            "Usage: mendimaru studio stop --session-id STUDIO_SESSION_ID\n\
             \n\
             Stops the selected Studio session through its verified process identity.",
        ),
        (Some("runtime"), None) => Some(
            "Usage: mendimaru runtime COMMAND\n\
             \n\
             Commands: build, start, list, status, wait, url, stop, forget, logs",
        ),
        (Some("runtime"), Some("build")) => Some(
            "Usage: mendimaru runtime build --project-id PROJECT_ID [--clean]\n\
             \n\
             Builds the portable Runtime package for the project's exact version.\n\
             --clean rebuilds only that project's cached package.",
        ),
        (Some("runtime"), Some("start")) => Some(
            "Usage: mendimaru runtime start --project-id PROJECT_ID [--clean] [--mode portable]\n\
            Usage: mendimaru runtime start --mode studio-run-locally\n\
                   [--studio-session-id STUDIO_SESSION_ID]\n\
             \n\
             Portable mode requires a project ID. Studio Run Locally uses the\n\
             Linux WinBoat adapter and optionally attaches to a live Studio session.",
        ),
        (Some("runtime"), Some("status")) => Some(
            "Usage: mendimaru runtime status --session-id RUNTIME_SESSION_ID\n\
             \n\
             Reports the Runtime session state and forwarded port information.",
        ),
        (Some("runtime"), Some("list")) => Some(
            "Usage: mendimaru runtime list\n\
             \n\
             Lists persisted Runtime session records without exposing paths.",
        ),
        (Some("runtime"), Some("wait")) => Some(
            "Usage: mendimaru runtime wait --session-id RUNTIME_SESSION_ID\n\
             \n\
             Waits until Runtime is HTTP-ready, fails, or reaches the CLI timeout.",
        ),
        (Some("runtime"), Some("url")) => Some(
            "Usage: mendimaru runtime url --session-id RUNTIME_SESSION_ID\n\
             \n\
             Returns a Runtime URL only after HTTP readiness is verified.",
        ),
        (Some("runtime"), Some("stop")) => Some(
            "Usage: mendimaru runtime stop --session-id RUNTIME_SESSION_ID\n\
             \n\
             Stops the Runtime session and restores any managed port forwarding.",
        ),
        (Some("runtime"), Some("forget")) => Some(
            "Usage: mendimaru runtime forget --session-id RUNTIME_SESSION_ID\n\
             \n\
             Explicitly invalidates a stopped, failed, or incompatible record.",
        ),
        (Some("runtime"), Some("logs")) => Some(
            "Usage: mendimaru runtime logs --session-id RUNTIME_SESSION_ID [--cursor CURSOR]\n\
             \n\
             Reads bounded diagnostic log entries. Secrets and raw paths are excluded.",
        ),
        (Some("browser"), None) => Some(
            "Usage: mendimaru browser COMMAND\n\
             \n\
             Commands: doctor, install chromium, test, artifacts",
        ),
        (Some("browser"), Some("doctor")) => Some(
            "Usage: mendimaru browser doctor\n\
             \n\
             Checks the browser test toolchain without installing anything.",
        ),
        (Some("browser"), Some("install")) => Some(
            "Usage: mendimaru browser install chromium\n\
             \n\
             Installs the pinned Chromium browser used by browser tests.",
        ),
        (Some("browser"), Some("test")) => Some(
            "Usage: mendimaru browser test (--base-url URL | --runtime-session-id ID)\n\
                    --suite-path SUITE_JSON [options]\n\
             \n\
             Options include timeout controls, --record-video, --record-har,\n\
             --fail-on-console-error, --fail-on-network-failure,\n\
             --max-artifact-mib, and --retention-runs. See browser-testing.md.",
        ),
        (Some("browser"), Some("artifacts")) => Some(
            "Usage: mendimaru browser artifacts --session-id BROWSER_SESSION_ID\n\
             \n\
             Exports retained artifacts for a browser test session.",
        ),
        (Some("project"), None) => Some(
            "Usage: mendimaru project COMMAND\n\
             \n\
             Commands: list, version",
        ),
        (Some("project"), Some("list")) => Some(
            "Usage: mendimaru project list\n\
             \n\
             Lists projects in the configured workspace using opaque project IDs.",
        ),
        (Some("project"), Some("version")) => Some(
            "Usage: mendimaru project version --project-id PROJECT_ID\n\
             \n\
             Resolves the exact Studio Pro version declared by a project.",
        ),
        (Some("operation"), None) => Some(
            "Usage: mendimaru operation COMMAND\n\
             \n\
             Commands: list, status, retry",
        ),
        (Some("operation"), Some("list")) => Some(
            "Usage: mendimaru operation list\n\
             \n\
             Lists persistent operation history.",
        ),
        (Some("operation"), Some("status")) => Some(
            "Usage: mendimaru operation status --operation-id OPERATION_ID\n\
             \n\
             Inspects one persistent operation and its safe diagnostic summary.",
        ),
        (Some("operation"), Some("retry")) => Some(
            "Usage: mendimaru operation retry --operation-id OPERATION_ID\n\
             \n\
             Retries only operations recorded as safely resumable.",
        ),
        _ => None,
    }
}

fn unknown_command_execution(arguments: &[OsString]) -> CliExecution {
    let error = BackendError::invalid_request(unknown_command_message(arguments));
    bare_error_execution("unknown", error)
}

fn unknown_command_message(arguments: &[OsString]) -> String {
    let values = arguments
        .iter()
        .filter_map(|value| value.to_str())
        .collect::<Vec<_>>();
    if values.contains(&"status") {
        return "unknown command; try 'env status' for environment status or \
                 'studio status' for Studio sessions"
            .to_string();
    }
    if values.iter().any(|&value| {
        matches!(
            value,
            "studio-status" | "studio_status" | "runtime-status" | "runtime_status"
        )
    }) {
        return "unknown command; Studio sessions use 'studio status' and Runtime sessions use \
                 'runtime status'"
            .to_string();
    }
    "unknown command; run 'mendimaru --help' to list supported commands".to_string()
}

fn root_help_execution() -> CliExecution {
    CliExecution {
        exit_code: EXIT_OK,
        stdout: format!("{}\n", ROOT_HELP),
        stderr: String::new(),
    }
}

const ROOT_HELP: &str = "\
Mendimaru headless CLI

Usage: mendimaru [--json | --ndjson] [--backend ID] [--timeout-seconds SECONDS] COMMAND

Commands:
  capabilities                     Print the backend capability snapshot
  env status                       Report the current environment status
  env ensure                       Ensure required environment dependencies are ready
  studio list                      List installed Studio Pro versions
  studio install --version VERSION Install an exact Studio Pro version
  studio uninstall --version VERSION
                                    Uninstall an exact Studio Pro version
  studio start --version VERSION  Start Studio Pro
  studio status [--session-id ID] [--refresh]
                                    Report Studio sessions
  studio status --orphans         Report authoritative sessions not owned by Mendimaru keepers
  studio stop --session-id ID      Stop a Studio session
  runtime build --project-id ID   Build a portable Runtime package
  runtime start [...]             Start Runtime (portable or Studio Run Locally)
  runtime list                    List persisted Runtime sessions
  runtime status --session-id ID  Report a Runtime session
  runtime wait --session-id ID    Wait for Runtime readiness
  runtime url --session-id ID     Print a readiness-verified Runtime URL
  runtime stop --session-id ID    Stop a Runtime session
  runtime forget --session-id ID  Forget a stopped or incompatible record
  runtime logs --session-id ID    Read bounded Runtime diagnostic logs
  browser doctor                  Check the browser test toolchain
  browser install chromium        Install the pinned Chromium test browser
  browser test [...]              Run a browser test suite
  browser artifacts --session-id ID
                                    Export retained browser test artifacts
  project list                    List projects in the configured workspace
  project version --project-id ID
                                    Resolve a project's exact Studio version
  operation list                  List persistent operations
  operation status --operation-id ID
                                    Inspect a persistent operation
  operation retry --operation-id ID
                                    Retry a safe terminal operation

Global options:
  --json                            Emit one JSON response document (default)
  --ndjson                          Emit progress as newline-delimited JSON
  --backend ID                      Select linux-winboat, windows-native, or mac-native
  --timeout-seconds SECONDS         Bound a command from 1 through 3600 seconds
  --snapshot                         Include the immutable capability snapshot
  --help, -h                        Show this help

Exit codes: 0 success, 1 operation failure, 2 invalid command, 3 backend unavailable.
See docs/headless-cli.md for command-specific options and response schemas.";

async fn run_command(
    command: &CliCommand,
    timeout: Duration,
    capability_snapshot: &CapabilitySnapshot,
) -> Result<CommandOutput, CommandError> {
    if matches!(command, CliCommand::Capabilities) {
        return CommandOutput::data(capability_snapshot);
    }
    let browser_capability = match command {
        CliCommand::BrowserDoctor
        | CliCommand::BrowserInstallChromium
        | CliCommand::BrowserTest { .. } => Some(CapabilityId::BrowserTest),
        CliCommand::BrowserArtifacts { .. } => Some(CapabilityId::BrowserArtifacts),
        _ => None,
    };
    if let Some(capability_id) = browser_capability {
        let capability = capability_snapshot
            .manifest
            .capability(capability_id)
            .ok_or_else(|| {
                CommandError::from(BackendError::unsupported(
                    capability_snapshot.manifest.backend,
                    capability_id,
                ))
            })?;
        if !capability_snapshot.manifest.supports(capability_id) {
            return Err(CommandError::from(BackendError::unsupported_with_reason(
                capability_snapshot.manifest.backend,
                capability_id,
                capability.limitation.clone().unwrap_or_else(|| {
                    crate::contracts::CapabilityLimitation::not_implemented(capability_id)
                }),
            )));
        }
    }
    match command {
        CliCommand::BrowserDoctor => {
            return CommandOutput::data(
                crate::application::browser_doctor(capability_snapshot.manifest.backend).await?,
            );
        }
        CliCommand::BrowserInstallChromium => {
            return CommandOutput::data(
                crate::application::browser_install_chromium(capability_snapshot.manifest.backend)
                    .await?,
            );
        }
        CliCommand::BrowserTest {
            base_url: Some(base_url),
            runtime_session_id: None,
            suite_path,
            policy,
        } => {
            return CommandOutput::data(
                crate::application::browser_test_url(
                    capability_snapshot.manifest.backend,
                    base_url,
                    suite_path,
                    policy.clone(),
                )
                .await?,
            );
        }
        CliCommand::BrowserArtifacts { session_id } => {
            return CommandOutput::data(crate::application::browser_artifacts(
                capability_snapshot.manifest.backend,
                session_id,
            )?);
        }
        _ => {}
    }
    let paths = AppPaths::discover_for_cli().map_err(|_| {
        CommandError::new(
            CommandErrorCode::ConfigLoadFailed,
            "the application directories could not be resolved".to_string(),
        )
    })?;
    let config = crate::application::load_config(&paths)?;
    match command {
        CliCommand::Capabilities => unreachable!("handled without configuration"),
        CliCommand::BrowserDoctor | CliCommand::BrowserInstallChromium => {
            unreachable!("handled without configuration")
        }
        CliCommand::EnvironmentStatus => {
            CommandOutput::data(crate::application::environment_status(&config).await)
        }
        CliCommand::EnvironmentEnsure => {
            CommandOutput::data(crate::application::ensure_environment(&config, timeout).await?)
        }
        CliCommand::StudioList => {
            CommandOutput::data(crate::application::installed_versions(&config).await?)
        }
        CliCommand::StudioInstall {
            version,
            force_redownload,
        } => {
            let cancellation = crate::downloads::DownloadCancellation::new();
            let mut progress = Vec::new();
            let operation_id = crate::application::install(
                &paths,
                &config,
                version.clone(),
                *force_redownload,
                None,
                &cancellation,
                |update| progress.push(update.clone()),
            )
            .await?;
            Ok(CommandOutput {
                data: json!({ "completed": true }),
                operation_id: Some(operation_id),
                studio_session_id: None,
                runtime_session_id: None,
                progress,
            })
        }
        CliCommand::StudioUninstall { version } => {
            let operation_id =
                crate::application::uninstall(&paths, &config, version.clone(), None).await?;
            Ok(CommandOutput {
                data: json!({ "completed": true }),
                operation_id: Some(operation_id),
                studio_session_id: None,
                runtime_session_id: None,
                progress: Vec::new(),
            })
        }
        CliCommand::StudioStart {
            version,
            project_id,
        } => {
            #[cfg(target_os = "linux")]
            let (operation_id, studio_session_id) =
                start_with_session_keeper(version, project_id.as_deref()).await?;
            #[cfg(not(target_os = "linux"))]
            let operation_id = if let Some(project_id) = project_id {
                crate::application::launch_project(&paths, &config, version.clone(), project_id)
                    .await?
            } else {
                crate::application::launch(&paths, &config, version.clone(), None, None).await?
            };
            #[cfg(not(target_os = "linux"))]
            let studio_session_id = None;
            Ok(CommandOutput {
                data: json!({ "completed": true }),
                operation_id: Some(operation_id),
                studio_session_id,
                runtime_session_id: None,
                progress: Vec::new(),
            })
        }
        CliCommand::StudioStatus {
            session_id,
            refresh,
            orphan_filter,
        } => {
            if *orphan_filter {
                #[cfg(target_os = "linux")]
                let owned = keeper_sessions(&paths).await?;
                #[cfg(not(target_os = "linux"))]
                let owned = Vec::new();
                let sessions = crate::application::studio_sessions(&config).await?;
                return CommandOutput::data(orphan_studio_sessions(sessions, &owned));
            }
            #[cfg(target_os = "linux")]
            {
                if let Some(session_id) = session_id {
                    if let Some(session) = keeper_session(&paths, session_id).await? {
                        let mut output = CommandOutput::data(&session)?;
                        output.studio_session_id = Some(session.session_id);
                        return Ok(output);
                    }
                } else {
                    let sessions = keeper_sessions(&paths).await?;
                    if !sessions.is_empty() || !*refresh {
                        return CommandOutput::data(sessions);
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = refresh;
            if let Some(session_id) = session_id {
                let session = crate::application::studio_session(&config, session_id).await?;
                let mut output = CommandOutput::data(&session)?;
                output.studio_session_id = Some(session.session_id);
                Ok(output)
            } else {
                CommandOutput::data(crate::application::studio_sessions(&config).await?)
            }
        }
        CliCommand::StudioStop { session_id } => {
            #[cfg(target_os = "linux")]
            let stopped_by_keeper = request_keeper_stop(&paths, session_id).await?;
            #[cfg(not(target_os = "linux"))]
            let stopped_by_keeper = false;
            if !stopped_by_keeper {
                if let Err(error) =
                    crate::application::stop_session(&paths, &config, session_id).await
                {
                    let sessions = crate::application::studio_sessions(&config).await;
                    return complete_stopped_session_if_absent(
                        error, sessions, &config, session_id,
                    )
                    .map(|()| CommandOutput {
                        data: json!({ "completed": true }),
                        operation_id: None,
                        studio_session_id: Some(session_id.clone()),
                        runtime_session_id: None,
                        progress: Vec::new(),
                    });
                }
            }
            Ok(CommandOutput {
                data: json!({ "completed": true }),
                operation_id: None,
                studio_session_id: Some(session_id.clone()),
                runtime_session_id: None,
                progress: Vec::new(),
            })
        }
        CliCommand::RuntimeBuild { project_id, clean } => CommandOutput::data(
            crate::application::runtime_build(&config, project_id, *clean).await?,
        ),
        CliCommand::RuntimeStart {
            project_id,
            clean,
            mode,
            studio_session_id,
            guest_port,
        } => {
            let (build, status) = crate::application::runtime_start(
                &config,
                project_id.as_deref(),
                *clean,
                *mode,
                studio_session_id.as_deref(),
                *guest_port,
                timeout,
            )
            .await?;
            let runtime_session_id = status.session_id.clone();
            let data = if let Some(build) = build {
                json!({ "build": build, "runtime": status })
            } else {
                json!({ "runtime": status })
            };
            Ok(CommandOutput {
                data,
                operation_id: None,
                studio_session_id: None,
                runtime_session_id: Some(runtime_session_id),
                progress: Vec::new(),
            })
        }
        CliCommand::RuntimeStatus { session_id } => {
            let status = crate::application::runtime_status(&config, session_id).await?;
            let mut output = CommandOutput::data(status)?;
            output.runtime_session_id = Some(session_id.clone());
            Ok(output)
        }
        CliCommand::RuntimeList => {
            CommandOutput::data(crate::application::runtime_sessions(&config).await?)
        }
        CliCommand::RuntimeWait { session_id } => {
            let status = crate::application::runtime_wait(&config, session_id).await?;
            let mut output = CommandOutput::data(status)?;
            output.runtime_session_id = Some(session_id.clone());
            Ok(output)
        }
        CliCommand::RuntimeUrl { session_id } => {
            let url = crate::application::runtime_url(&config, session_id).await?;
            let mut output = CommandOutput::data(json!({ "url": url }))?;
            output.runtime_session_id = Some(session_id.clone());
            Ok(output)
        }
        CliCommand::RuntimeStop { session_id } => {
            crate::application::runtime_stop(&config, session_id).await?;
            let mut output = CommandOutput::data(json!({ "completed": true }))?;
            output.runtime_session_id = Some(session_id.clone());
            Ok(output)
        }
        CliCommand::RuntimeForget { session_id } => {
            let result = crate::application::runtime_forget(&config, session_id).await?;
            let mut output = CommandOutput::data(result)?;
            output.runtime_session_id = Some(session_id.clone());
            Ok(output)
        }
        CliCommand::RuntimeLogs { session_id, cursor } => {
            let logs =
                crate::application::runtime_logs(&config, session_id, cursor.as_deref()).await?;
            let mut output = CommandOutput::data(logs)?;
            output.runtime_session_id = Some(session_id.clone());
            Ok(output)
        }
        CliCommand::BrowserTest {
            base_url: None,
            runtime_session_id: Some(runtime_session_id),
            suite_path,
            policy,
        } => CommandOutput::data(
            crate::application::browser_test_runtime(
                &config,
                runtime_session_id,
                suite_path,
                policy.clone(),
            )
            .await?,
        ),
        CliCommand::BrowserTest { .. } => {
            unreachable!("browser test target invariant was validated")
        }
        CliCommand::BrowserArtifacts { .. } => unreachable!("handled without configuration"),
        CliCommand::ProjectList => CommandOutput::data(crate::application::projects(&config)?),
        CliCommand::ProjectVersion { project_id } => {
            let project = crate::application::project(&config, project_id)?;
            CommandOutput::data(json!({
                "projectId": project.project_id,
                "requiredVersion": project.required_version,
            }))
        }
        CliCommand::OperationList => {
            CommandOutput::data(crate::application::operations(&paths, &config)?)
        }
        CliCommand::OperationStatus { operation_id } => {
            let operation = crate::application::operation(&paths, &config, operation_id)?;
            let mut output = CommandOutput::data(&operation)?;
            output.operation_id = Some(operation.id);
            Ok(output)
        }
        CliCommand::OperationRetry { operation_id } => {
            let cancellation = crate::downloads::DownloadCancellation::new();
            let mut progress = Vec::new();
            let new_operation_id =
                crate::application::retry(&paths, &config, &cancellation, operation_id, |update| {
                    progress.push(update.clone())
                })
                .await?;
            Ok(CommandOutput {
                data: json!({ "completed": true, "retryOf": operation_id }),
                operation_id: Some(new_operation_id),
                studio_session_id: None,
                runtime_session_id: None,
                progress,
            })
        }
    }
}

fn orphan_studio_sessions(
    authoritative: Vec<crate::contracts::StudioSessionStatus>,
    owned: &[crate::contracts::StudioSessionStatus],
) -> Vec<crate::contracts::StudioSessionStatus> {
    authoritative
        .into_iter()
        .filter(|session| {
            !owned
                .iter()
                .any(|keeper| keeper.session_id == session.session_id)
        })
        .collect()
}

fn complete_stopped_session_if_absent(
    error: CommandError,
    sessions: Result<Vec<crate::contracts::StudioSessionStatus>, CommandError>,
    config: &crate::models::AppConfig,
    session_id: &str,
) -> Result<(), CommandError> {
    if matches!(error.code, CommandErrorCode::InvalidRequest) {
        return Err(error);
    }
    let sessions = match sessions {
        Ok(sessions) => sessions,
        Err(_) => return Err(error),
    };
    if sessions
        .iter()
        .any(|session| session.session_id == session_id)
    {
        return Err(error);
    }
    crate::winboat::cleanup_dead_session_lock(config, session_id);
    Ok(())
}

#[cfg(target_os = "linux")]
async fn start_with_session_keeper(
    version: &str,
    project_id: Option<&str>,
) -> Result<(String, Option<String>), CommandError> {
    use std::os::unix::process::CommandExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let executable = std::env::current_exe().map_err(|_| {
        CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper executable could not be resolved".to_string(),
        )
    })?;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("__session-keeper")
        .arg("--version")
        .arg(version)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    if let Some(project_id) = project_id {
        command.arg("--project-id").arg(project_id);
    }
    // The keeper owns the RemoteApp connection after this CLI invocation
    // exits. setsid prevents a terminal hangup from coupling it back to the
    // caller's process group; only the verified Studio session controls its
    // lifetime after the handshake.
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|_| {
        CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper could not be started".to_string(),
        )
    })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper acknowledgement is unavailable".to_string(),
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper handshake is unavailable".to_string(),
        )
    })?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let count = reader.read_line(&mut line).await.map_err(|_| {
        CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper handshake failed".to_string(),
        )
    })?;
    if count == 0 {
        return Err(CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper exited before launch completed".to_string(),
        ));
    }
    let response = serde_json::from_str::<SessionKeeperResponse>(&line).map_err(|_| {
        CommandError::new(
            CommandErrorCode::OperationFailed,
            "the session keeper returned an invalid handshake".to_string(),
        )
    })?;
    if response.ok {
        let operation_id = response.operation_id.ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::OperationFailed,
                "the session keeper omitted the operation ID".to_string(),
            )
        })?;
        stdin.write_all(b"accept\n").await.map_err(|_| {
            CommandError::new(
                CommandErrorCode::OperationFailed,
                "the session keeper acknowledgement failed".to_string(),
            )
        })?;
        stdin.flush().await.map_err(|_| {
            CommandError::new(
                CommandErrorCode::OperationFailed,
                "the session keeper acknowledgement failed".to_string(),
            )
        })?;
        Ok((operation_id, response.studio_session_id))
    } else {
        Err(response
            .error
            .unwrap_or_else(|| {
                BackendError::operation(
                    BackendId::LinuxWinboat,
                    CapabilityId::StudioStart,
                    "the session keeper launch failed",
                )
            })
            .into())
    }
}

#[cfg(target_os = "linux")]
fn session_keeper_dispatch(arguments: &[OsString]) -> i32 {
    use std::io::{BufRead, Read};

    let mut response = session_keeper_response(arguments);
    let completed_operation_id = if response.ok {
        response.operation_id.clone()
    } else {
        None
    };
    let prepared = if response.ok {
        response
            .studio_session_id
            .as_deref()
            .ok_or_else(|| "the launched Studio session ID is unavailable".to_string())
            .and_then(prepare_session_keeper)
    } else {
        Err("the Studio session keeper launch failed".to_string())
    };
    if let Err(message) = &prepared {
        if response.ok {
            response = keeper_error(BackendError::operation(
                BackendId::LinuxWinboat,
                CapabilityId::StudioStart,
                message,
            ));
        }
    }
    let successful = response.ok;
    let serialized = serde_json::to_vec(&response).unwrap_or_else(|_| {
        br#"{"ok":false,"error":{"schemaVersion":"4.0.0","code":"operation_failed","message":"session keeper serialization failed","retryable":false}}"#.to_vec()
    });
    let written = std::io::stdout()
        .write_all(&serialized)
        .and_then(|()| std::io::stdout().write_all(b"\n"))
        .and_then(|()| std::io::stdout().flush())
        .is_ok();
    let mut acknowledgement = String::new();
    let accepted = successful
        && written
        && std::io::stdin()
            .lock()
            .take(16)
            .read_line(&mut acknowledgement)
            .is_ok_and(|count| count > 0)
        && acknowledgement.trim_end() == "accept";
    if accepted {
        let (listener, socket_guard, session_id) =
            prepared.expect("a successful keeper response has a prepared listener");
        tauri::async_runtime::block_on(serve_session_keeper(listener, socket_guard, &session_id));
        EXIT_OK
    } else {
        if let (Some(operation_id), Ok(paths)) =
            (completed_operation_id, AppPaths::discover_for_cli())
        {
            let _ = crate::operations::interrupt_completed_launch_with_paths(&paths, &operation_id);
        }
        tauri::async_runtime::block_on(crate::winboat::close_all_registered_clients());
        EXIT_OPERATION_FAILED
    }
}

#[cfg(target_os = "linux")]
fn session_keeper_response(arguments: &[OsString]) -> SessionKeeperResponse {
    if crate::i18n::initialize("en-US").is_err() {
        return keeper_error(BackendError::operation(
            BackendId::LinuxWinboat,
            CapabilityId::StudioStart,
            "localization initialization failed",
        ));
    }
    let values = match arguments
        .iter()
        .map(|value| value.to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
    {
        Some(values) => values,
        None => return keeper_error(BackendError::invalid_request("invalid keeper arguments")),
    };
    let (options, _) = match parse_options(&values, &["--version", "--project-id"], &[]) {
        Ok(parsed) => parsed,
        Err(error) => return keeper_error(sanitize_backend_error(error)),
    };
    let Some(version) = options.get("--version").cloned() else {
        return keeper_error(BackendError::invalid_request(
            "the keeper version is missing",
        ));
    };
    let paths = match AppPaths::discover_for_cli() {
        Ok(paths) => paths,
        Err(_) => {
            return keeper_error(BackendError::operation(
                BackendId::LinuxWinboat,
                CapabilityId::StudioStart,
                "the application directories could not be resolved",
            ))
        }
    };
    let config = match crate::application::load_config(&paths) {
        Ok(config) => config,
        Err(error) => {
            return keeper_error(command_error_to_backend(error, BackendId::LinuxWinboat))
        }
    };
    let result = tauri::async_runtime::block_on(async {
        if let Some(project_id) = options.get("--project-id") {
            crate::application::launch_project(&paths, &config, version, project_id).await
        } else {
            crate::application::launch(&paths, &config, version, None, None).await
        }
    });
    match result {
        Ok(operation_id) => {
            let sessions = crate::winboat::registered_client_sessions();
            if sessions.len() != 1 {
                return keeper_error(BackendError::operation(
                    BackendId::LinuxWinboat,
                    CapabilityId::StudioStart,
                    "the launched Studio session could not be registered",
                ));
            }
            SessionKeeperResponse {
                ok: true,
                operation_id: Some(operation_id),
                studio_session_id: Some(sessions[0].session_id.clone()),
                error: None,
            }
        }
        Err(error) => keeper_error(command_error_to_backend(error, BackendId::LinuxWinboat)),
    }
}

#[cfg(target_os = "linux")]
fn keeper_error(error: BackendError) -> SessionKeeperResponse {
    SessionKeeperResponse {
        ok: false,
        operation_id: None,
        studio_session_id: None,
        error: Some(sanitize_backend_error(error)),
    }
}

#[cfg(target_os = "linux")]
struct SessionSocketGuard {
    path: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionSocketCleanupReport {
    schema_version: &'static str,
    removed_sockets: u32,
    retained_live_sockets: u32,
    observed_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(target_os = "linux")]
impl Drop for SessionSocketGuard {
    fn drop(&mut self) {
        let Ok(metadata) = std::fs::symlink_metadata(&self.path) else {
            return;
        };
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() } {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(target_os = "linux")]
fn cleanup_stale_session_keeper_sockets(
    directory: &std::path::Path,
    preserve: &std::path::Path,
) -> Result<u32, String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    const MAX_SOCKETS: usize = 128;
    const MAX_AUDIT_BYTES: u64 = 64 * 1024;
    let mut removed_sockets = 0_u32;
    let mut retained_live_sockets = 0_u32;
    let entries = std::fs::read_dir(directory)
        .map_err(|_| "the session keeper directory could not be read".to_string())?;
    for entry in entries.take(MAX_SOCKETS) {
        let entry =
            entry.map_err(|_| "a session keeper directory entry could not be read".to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("s-") || !name.ends_with(".sock") {
            continue;
        }
        let path = entry.path();
        if path == preserve {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_socket()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            continue;
        }
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            retained_live_sockets += 1;
            continue;
        }
        std::fs::remove_file(&path)
            .map_err(|_| "a stale session keeper socket could not be removed".to_string())?;
        removed_sockets += 1;
    }
    if removed_sockets == 0 && retained_live_sockets == 0 {
        return Ok(0);
    }
    let report = SessionSocketCleanupReport {
        schema_version: CONTRACT_SCHEMA_VERSION,
        removed_sockets,
        retained_live_sockets,
        observed_at: chrono::Utc::now(),
    };
    let audit_path = directory.join("socket-cleanup.log");
    match std::fs::symlink_metadata(&audit_path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() >= MAX_AUDIT_BYTES
            {
                return Err("the session keeper cleanup audit is unavailable".to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err("the session keeper cleanup audit could not be inspected".to_string());
        }
    }
    let mut payload = serde_json::to_vec(&report)
        .map_err(|_| "the session keeper cleanup audit could not be serialized".to_string())?;
    payload.push(b'\n');
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    let mut file = options
        .open(audit_path)
        .map_err(|_| "the session keeper cleanup audit could not be opened".to_string())?;
    file.write_all(&payload)
        .and_then(|()| file.flush())
        .map_err(|_| "the session keeper cleanup audit could not be written".to_string())?;
    Ok(removed_sockets)
}

#[cfg(target_os = "linux")]
fn prepare_session_keeper(
    session_id: &str,
) -> Result<(std::os::unix::net::UnixListener, SessionSocketGuard, String), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    let paths = AppPaths::discover_for_cli()
        .map_err(|_| "the session keeper directory could not be resolved".to_string())?;
    let directory = ensure_session_socket_directory(&paths)?;
    let socket_path = directory.join(session_socket_name(session_id));
    cleanup_stale_session_keeper_sockets(&directory, &socket_path)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&socket_path) {
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("the session keeper socket is not trusted".to_string());
        }
        if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
            return Err("the Studio session already has a live keeper".to_string());
        }
        std::fs::remove_file(&socket_path)
            .map_err(|_| "a stale session keeper socket could not be removed".to_string())?;
    }
    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .map_err(|_| "the session keeper socket could not be created".to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "the session keeper socket could not be configured".to_string())?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| "the session keeper socket permissions could not be secured".to_string())?;
    Ok((
        listener,
        SessionSocketGuard { path: socket_path },
        session_id.to_string(),
    ))
}

#[cfg(target_os = "linux")]
fn ensure_session_socket_directory(paths: &AppPaths) -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    paths
        .ensure_cache_directory()
        .map_err(|_| "the application cache directory is unavailable".to_string())?;
    let directory = paths.cache_directory().join("cli-sessions");
    std::fs::create_dir_all(&directory)
        .map_err(|_| "the session keeper directory could not be created".to_string())?;
    let metadata = std::fs::symlink_metadata(&directory)
        .map_err(|_| "the session keeper directory could not be inspected".to_string())?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("the session keeper directory is not trusted".to_string());
    }
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "the session keeper directory permissions could not be secured".to_string())?;
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn session_socket_name(session_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(session_id.as_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("s-{suffix}.sock")
}

#[cfg(target_os = "linux")]
async fn serve_session_keeper(
    listener: std::os::unix::net::UnixListener,
    _socket_guard: SessionSocketGuard,
    session_id: &str,
) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let Ok(listener) = tokio::net::UnixListener::from_std(listener) else {
        cleanup_linked_runtimes(session_id).await;
        crate::winboat::close_all_registered_clients().await;
        return;
    };
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    cleanup_linked_runtimes(session_id).await;
                    crate::winboat::close_all_registered_clients().await;
                    return;
                };
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half).take(32);
                let mut request = String::new();
                let read = tokio::time::timeout(
                    Duration::from_secs(2),
                    reader.read_line(&mut request),
                ).await;
                let mut should_stop = false;
                let response = if matches!(read, Ok(Ok(count)) if count > 0)
                    && request.trim_end() == "status"
                {
                    SessionKeeperIpcResponse {
                        ok: true,
                        session: crate::winboat::registered_client_sessions()
                            .into_iter()
                            .find(|session| session.session_id == session_id),
                    }
                } else if matches!(read, Ok(Ok(count)) if count > 0)
                    && request.trim_end() == "stop"
                {
                    should_stop = crate::winboat::stop_registered_client(session_id)
                        .await
                        .unwrap_or(false);
                    SessionKeeperIpcResponse {
                        ok: should_stop,
                        session: None,
                    }
                } else {
                    SessionKeeperIpcResponse {
                        ok: false,
                        session: None,
                    }
                };
                if let Ok(mut payload) = serde_json::to_vec(&response) {
                    payload.push(b'\n');
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        write_half.write_all(&payload),
                    ).await;
                }
                if should_stop {
                    cleanup_linked_runtimes(session_id).await;
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if !crate::winboat::registered_client_sessions()
                    .iter()
                    .any(|session| session.session_id == session_id)
                {
                    cleanup_linked_runtimes(session_id).await;
                    return;
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
async fn cleanup_linked_runtimes(session_id: &str) {
    let Ok(paths) = AppPaths::discover_for_cli() else {
        return;
    };
    let Ok(config) = crate::application::load_config(&paths) else {
        return;
    };
    let _ = crate::winboat::runtime::stop_for_studio_session(&config, session_id).await;
}

#[cfg(target_os = "linux")]
async fn keeper_sessions(
    paths: &AppPaths,
) -> Result<Vec<crate::contracts::StudioSessionStatus>, CommandError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let directory = ensure_session_socket_directory(paths).map_err(keeper_command_error)?;
    let entries = std::fs::read_dir(directory).map_err(|_| {
        keeper_command_error("the session keeper directory could not be read".to_string())
    })?;
    let mut sessions = Vec::new();
    for entry in entries.take(64) {
        let Ok(entry) = entry else { continue };
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            continue;
        }
        if let Some(response) = request_session_keeper(&entry.path(), "status").await? {
            if response.ok {
                if let Some(session) = response.session {
                    if !sessions
                        .iter()
                        .any(|existing: &crate::contracts::StudioSessionStatus| {
                            existing.session_id == session.session_id
                        })
                    {
                        sessions.push(session);
                    }
                }
            }
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.started_at));
    Ok(sessions)
}

#[cfg(target_os = "linux")]
async fn keeper_session(
    paths: &AppPaths,
    session_id: &str,
) -> Result<Option<crate::contracts::StudioSessionStatus>, CommandError> {
    let directory = ensure_session_socket_directory(paths).map_err(keeper_command_error)?;
    let socket_path = directory.join(session_socket_name(session_id));
    let response = request_session_keeper(&socket_path, "status").await?;
    let Some(response) = response else {
        return Ok(None);
    };
    let session = response
        .session
        .filter(|session| session.session_id == session_id);
    if response.ok && session.is_some() {
        Ok(session)
    } else {
        Err(keeper_command_error(
            "the session keeper returned an invalid status".to_string(),
        ))
    }
}

#[cfg(target_os = "linux")]
async fn request_keeper_stop(paths: &AppPaths, session_id: &str) -> Result<bool, CommandError> {
    let directory = ensure_session_socket_directory(paths).map_err(keeper_command_error)?;
    let socket_path = directory.join(session_socket_name(session_id));
    let Some(response) = request_session_keeper(&socket_path, "stop").await? else {
        return Ok(false);
    };
    Ok(response.ok && response.session.is_none())
}

#[cfg(target_os = "linux")]
async fn request_session_keeper(
    socket_path: &std::path::Path,
    request: &str,
) -> Result<Option<SessionKeeperIpcResponse>, CommandError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(keeper_command_error(
                "the session keeper socket could not be inspected".to_string(),
            ))
        }
    };
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(keeper_command_error(
            "the session keeper socket is not trusted".to_string(),
        ));
    }
    let stream = match tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(socket_path),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            return Ok(None)
        }
        _ => {
            return Err(keeper_command_error(
                "the session keeper could not be reached".to_string(),
            ))
        }
    };
    let (read_half, mut write_half) = stream.into_split();
    let payload = format!("{request}\n");
    tokio::time::timeout(
        Duration::from_secs(2),
        write_half.write_all(payload.as_bytes()),
    )
    .await
    .map_err(|_| keeper_command_error("the session keeper request timed out".to_string()))?
    .map_err(|_| keeper_command_error("the session keeper request failed".to_string()))?;
    let mut reader = BufReader::new(read_half).take(32 * 1024);
    let mut response = String::new();
    let count = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut response))
        .await
        .map_err(|_| keeper_command_error("the session keeper response timed out".to_string()))?
        .map_err(|_| keeper_command_error("the session keeper response failed".to_string()))?;
    if count == 0 {
        return Err(keeper_command_error(
            "the session keeper response was empty".to_string(),
        ));
    }
    serde_json::from_str(&response)
        .map(Some)
        .map_err(|_| keeper_command_error("the session keeper response was invalid".to_string()))
}

#[cfg(target_os = "linux")]
fn keeper_command_error(message: String) -> CommandError {
    CommandError::new(CommandErrorCode::OperationFailed, message)
}

fn parse(arguments: &[OsString]) -> Result<ParsedCli, BackendError> {
    let values = arguments
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| BackendError::invalid_request("CLI arguments must be valid UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut format = OutputFormat::Json;
    let mut format_seen = false;
    let mut backend = None;
    let mut timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
    let mut timeout_seen = false;
    let mut include_snapshot = false;
    let mut snapshot_seen = false;
    let mut command_values = Vec::new();
    let mut index = 0;
    while index < values.len() {
        match values[index].as_str() {
            "--json" | "--ndjson" => {
                if format_seen {
                    return Err(BackendError::invalid_request(
                        "only one output format may be selected",
                    ));
                }
                format_seen = true;
                format = if values[index] == "--ndjson" {
                    OutputFormat::Ndjson
                } else {
                    OutputFormat::Json
                };
            }
            "--snapshot" => {
                if snapshot_seen {
                    return Err(BackendError::invalid_request(
                        "--snapshot may only be provided once",
                    ));
                }
                snapshot_seen = true;
                include_snapshot = true;
            }
            "--backend" => {
                if backend.is_some() {
                    return Err(BackendError::invalid_request(
                        "--backend may only be provided once",
                    ));
                }
                index += 1;
                let value = values.get(index).ok_or_else(|| {
                    BackendError::invalid_request("--backend requires a backend ID")
                })?;
                backend = Some(parse_backend(value)?);
            }
            value if value.starts_with("--backend=") => {
                if backend.is_some() {
                    return Err(BackendError::invalid_request(
                        "--backend may only be provided once",
                    ));
                }
                backend = Some(parse_backend(value.trim_start_matches("--backend="))?);
            }
            "--timeout-seconds" => {
                if timeout_seen {
                    return Err(BackendError::invalid_request(
                        "--timeout-seconds may only be provided once",
                    ));
                }
                timeout_seen = true;
                index += 1;
                let value = values.get(index).ok_or_else(|| {
                    BackendError::invalid_request("--timeout-seconds requires an integer")
                })?;
                timeout_seconds = parse_timeout(value)?;
            }
            value if value.starts_with("--timeout-seconds=") => {
                if timeout_seen {
                    return Err(BackendError::invalid_request(
                        "--timeout-seconds may only be provided once",
                    ));
                }
                timeout_seen = true;
                timeout_seconds = parse_timeout(value.trim_start_matches("--timeout-seconds="))?;
            }
            _ => command_values.push(values[index].clone()),
        }
        index += 1;
    }
    let command = parse_command(&command_values)?;
    Ok(ParsedCli {
        command,
        backend,
        format,
        timeout: Duration::from_secs(timeout_seconds),
        include_snapshot,
    })
}

fn parse_command(values: &[String]) -> Result<CliCommand, BackendError> {
    match values.first().map(String::as_str) {
        Some("capabilities") if values.len() == 1 => Ok(CliCommand::Capabilities),
        Some("env") => match values.get(1).map(String::as_str) {
            Some("status") if values.len() == 2 => Ok(CliCommand::EnvironmentStatus),
            Some("ensure") if values.len() == 2 => Ok(CliCommand::EnvironmentEnsure),
            _ => Err(BackendError::invalid_request(
                "expected env status or env ensure",
            )),
        },
        Some("studio") => parse_studio_command(&values[1..]),
        Some("runtime") => parse_runtime_command(&values[1..]),
        Some("browser") => parse_browser_command(&values[1..]),
        Some("project") => match values.get(1).map(String::as_str) {
            Some("list") if values.len() == 2 => Ok(CliCommand::ProjectList),
            Some("version") => Ok(CliCommand::ProjectVersion {
                project_id: required_option(&values[2..], "--project-id")?,
            }),
            _ => Err(BackendError::invalid_request(
                "expected project list or project version",
            )),
        },
        Some("operation") => match values.get(1).map(String::as_str) {
            Some("list") if values.len() == 2 => Ok(CliCommand::OperationList),
            Some("status") => Ok(CliCommand::OperationStatus {
                operation_id: required_option(&values[2..], "--operation-id")?,
            }),
            Some("retry") => Ok(CliCommand::OperationRetry {
                operation_id: required_option(&values[2..], "--operation-id")?,
            }),
            _ => Err(BackendError::invalid_request(
                "expected operation list, operation status, or operation retry",
            )),
        },
        Some("capabilities") => Err(BackendError::invalid_request(
            "capabilities does not accept positional arguments",
        )),
        _ => Err(BackendError::invalid_request("unknown headless command")),
    }
}

fn parse_browser_command(values: &[String]) -> Result<CliCommand, BackendError> {
    match values.first().map(String::as_str) {
        Some("doctor") if values.len() == 1 => Ok(CliCommand::BrowserDoctor),
        Some("install")
            if values.get(1).map(String::as_str) == Some("chromium") && values.len() == 2 =>
        {
            Ok(CliCommand::BrowserInstallChromium)
        }
        Some("artifacts") => Ok(CliCommand::BrowserArtifacts {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        Some("test") => {
            let (options, flags) = parse_options(
                &values[1..],
                &[
                    "--base-url",
                    "--runtime-session-id",
                    "--suite-path",
                    "--navigation-timeout-ms",
                    "--action-timeout-ms",
                    "--assertion-timeout-ms",
                    "--max-artifact-mib",
                    "--retention-runs",
                ],
                &[
                    "--fail-on-console-error",
                    "--fail-on-network-failure",
                    "--record-video",
                    "--record-har",
                ],
            )?;
            let base_url = options.get("--base-url").cloned();
            let runtime_session_id = options.get("--runtime-session-id").cloned();
            if base_url.is_some() == runtime_session_id.is_some() {
                return Err(BackendError::invalid_request(
                    "exactly one of --base-url or --runtime-session-id is required",
                ));
            }
            let policy = BrowserTestPolicy {
                navigation_timeout_milliseconds: options
                    .get("--navigation-timeout-ms")
                    .map(|value| parse_browser_timeout(value))
                    .transpose()?
                    .unwrap_or(DEFAULT_BROWSER_NAVIGATION_TIMEOUT_MS),
                action_timeout_milliseconds: options
                    .get("--action-timeout-ms")
                    .map(|value| parse_browser_timeout(value))
                    .transpose()?
                    .unwrap_or(DEFAULT_BROWSER_ACTION_TIMEOUT_MS),
                assertion_timeout_milliseconds: options
                    .get("--assertion-timeout-ms")
                    .map(|value| parse_browser_timeout(value))
                    .transpose()?
                    .unwrap_or(DEFAULT_BROWSER_ASSERTION_TIMEOUT_MS),
                fail_on_console_error: flags.contains("--fail-on-console-error"),
                fail_on_network_failure: flags.contains("--fail-on-network-failure"),
                record_video: flags.contains("--record-video"),
                record_har: flags.contains("--record-har"),
                max_artifact_bytes: options
                    .get("--max-artifact-mib")
                    .map(|value| parse_artifact_mebibytes(value))
                    .transpose()?
                    .unwrap_or(DEFAULT_BROWSER_ARTIFACT_MIB * 1024 * 1024),
                retention_runs: options
                    .get("--retention-runs")
                    .map(|value| parse_retention_runs(value))
                    .transpose()?
                    .unwrap_or(DEFAULT_BROWSER_RETENTION_RUNS),
            };
            Ok(CliCommand::BrowserTest {
                base_url,
                runtime_session_id,
                suite_path: required_map_option(&options, "--suite-path")?,
                policy,
            })
        }
        _ => Err(BackendError::invalid_request(
            "expected browser doctor, install chromium, test, or artifacts",
        )),
    }
}

fn parse_runtime_command(values: &[String]) -> Result<CliCommand, BackendError> {
    match values.first().map(String::as_str) {
        Some("build") => {
            let (options, flags) = parse_options(&values[1..], &["--project-id"], &["--clean"])?;
            let project_id = required_map_option(&options, "--project-id")?;
            let clean = flags.contains("--clean");
            Ok(CliCommand::RuntimeBuild { project_id, clean })
        }
        Some("start") => {
            let (options, flags) = parse_options(
                &values[1..],
                &["--project-id", "--mode", "--studio-session-id"],
                &["--clean"],
            )?;
            let mode = options
                .get("--mode")
                .map(|value| parse_runtime_mode(value))
                .transpose()?
                .unwrap_or(RuntimeMode::Portable);
            let project_id = options.get("--project-id").cloned();
            let studio_session_id = options.get("--studio-session-id").cloned();
            let clean = flags.contains("--clean");
            match mode {
                RuntimeMode::Portable if project_id.is_none() => {
                    return Err(BackendError::invalid_request(
                        "--project-id is required for portable Runtime mode",
                    ));
                }
                RuntimeMode::Portable if studio_session_id.is_some() => {
                    return Err(BackendError::invalid_request(
                        "--studio-session-id requires studio-run-locally mode",
                    ));
                }
                RuntimeMode::StudioRunLocally if project_id.is_some() || clean => {
                    return Err(BackendError::invalid_request(
                        "--project-id and --clean are not accepted in studio-run-locally mode",
                    ));
                }
                RuntimeMode::ExternalUrl => {
                    return Err(BackendError::invalid_request(
                        "external-url Runtime sessions cannot be started",
                    ));
                }
                _ => {}
            }
            Ok(CliCommand::RuntimeStart {
                project_id,
                clean,
                mode,
                studio_session_id,
                guest_port: None,
            })
        }
        Some("status") => Ok(CliCommand::RuntimeStatus {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        Some("list") if values.len() == 1 => Ok(CliCommand::RuntimeList),
        Some("wait") => Ok(CliCommand::RuntimeWait {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        Some("url") => Ok(CliCommand::RuntimeUrl {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        Some("stop") => Ok(CliCommand::RuntimeStop {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        Some("forget") => Ok(CliCommand::RuntimeForget {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        Some("logs") => {
            let (options, _) = parse_options(&values[1..], &["--session-id", "--cursor"], &[])?;
            Ok(CliCommand::RuntimeLogs {
                session_id: required_map_option(&options, "--session-id")?,
                cursor: options.get("--cursor").cloned(),
            })
        }
        _ => Err(BackendError::invalid_request(
            "expected runtime build, start, list, status, wait, url, stop, forget, or logs",
        )),
    }
}

fn parse_studio_command(values: &[String]) -> Result<CliCommand, BackendError> {
    match values.first().map(String::as_str) {
        Some("list") if values.len() == 1 => Ok(CliCommand::StudioList),
        Some("install") => {
            let (options, flags) =
                parse_options(&values[1..], &["--version"], &["--force-redownload"])?;
            Ok(CliCommand::StudioInstall {
                version: required_map_option(&options, "--version")?,
                force_redownload: flags.contains("--force-redownload"),
            })
        }
        Some("uninstall") => Ok(CliCommand::StudioUninstall {
            version: required_option(&values[1..], "--version")?,
        }),
        Some("start") => {
            let (options, _) = parse_options(&values[1..], &["--version", "--project-id"], &[])?;
            Ok(CliCommand::StudioStart {
                version: required_map_option(&options, "--version")?,
                project_id: options.get("--project-id").cloned(),
            })
        }
        Some("status") => {
            let (options, flags) =
                parse_options(&values[1..], &["--session-id"], &["--refresh", "--orphans"])?;
            let orphan_filter = flags.contains("--orphans");
            if orphan_filter && options.contains_key("--session-id") {
                return Err(BackendError::invalid_request(
                    "--session-id cannot be combined with --orphans",
                ));
            }
            Ok(CliCommand::StudioStatus {
                session_id: options.get("--session-id").cloned(),
                refresh: flags.contains("--refresh"),
                orphan_filter,
            })
        }
        Some("stop") => Ok(CliCommand::StudioStop {
            session_id: required_option(&values[1..], "--session-id")?,
        }),
        _ => Err(BackendError::invalid_request(
            "expected studio list, install, uninstall, start, status, or stop",
        )),
    }
}

fn required_option(values: &[String], name: &str) -> Result<String, BackendError> {
    let (options, _) = parse_options(values, &[name], &[])?;
    required_map_option(&options, name)
}

fn required_map_option(
    options: &std::collections::BTreeMap<String, String>,
    name: &str,
) -> Result<String, BackendError> {
    options
        .get(name)
        .cloned()
        .ok_or_else(|| BackendError::invalid_request(format!("{name} is required")))
}

fn parse_options(
    values: &[String],
    option_names: &[&str],
    flag_names: &[&str],
) -> Result<
    (
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeSet<String>,
    ),
    BackendError,
> {
    let mut options = std::collections::BTreeMap::new();
    let mut flags = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < values.len() {
        let value = &values[index];
        if flag_names.contains(&value.as_str()) {
            if !flags.insert(value.clone()) {
                return Err(BackendError::invalid_request("an option was duplicated"));
            }
        } else if option_names.contains(&value.as_str()) {
            index += 1;
            let option_value = values
                .get(index)
                .filter(|candidate| !candidate.starts_with("--"))
                .ok_or_else(|| BackendError::invalid_request("an option value is missing"))?;
            if options
                .insert(value.clone(), option_value.clone())
                .is_some()
            {
                return Err(BackendError::invalid_request("an option was duplicated"));
            }
        } else if let Some((name, option_value)) = value.split_once('=') {
            if !option_names.contains(&name) || option_value.is_empty() {
                return Err(BackendError::invalid_request("an option is invalid"));
            }
            if options
                .insert(name.to_string(), option_value.to_string())
                .is_some()
            {
                return Err(BackendError::invalid_request("an option was duplicated"));
            }
        } else {
            return Err(BackendError::invalid_request(
                "an unknown option was provided",
            ));
        }
        index += 1;
    }
    Ok((options, flags))
}

fn parse_backend(value: &str) -> Result<BackendId, BackendError> {
    BackendId::from_str(value).map_err(|_| {
        BackendError::invalid_request(
            "unknown backend; expected linux-winboat, windows-native, or mac-native",
        )
    })
}

fn parse_timeout(value: &str) -> Result<u64, BackendError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|seconds| (1..=MAX_TIMEOUT_SECONDS).contains(seconds))
        .ok_or_else(|| {
            BackendError::invalid_request("timeout must be an integer from 1 through 3600")
        })
}

fn parse_browser_timeout(value: &str) -> Result<u64, BackendError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|milliseconds| (100..=300_000).contains(milliseconds))
        .ok_or_else(|| {
            BackendError::invalid_request(
                "browser timeout must be an integer from 100 through 300000 milliseconds",
            )
        })
}

fn parse_artifact_mebibytes(value: &str) -> Result<u64, BackendError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|mebibytes| (1..=512).contains(mebibytes))
        .and_then(|mebibytes| mebibytes.checked_mul(1024 * 1024))
        .ok_or_else(|| {
            BackendError::invalid_request(
                "browser artifact limit must be an integer from 1 through 512 MiB",
            )
        })
}

fn parse_retention_runs(value: &str) -> Result<u32, BackendError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|runs| (1..=100).contains(runs))
        .ok_or_else(|| {
            BackendError::invalid_request(
                "browser retention must be an integer from 1 through 100 runs",
            )
        })
}

fn parse_runtime_mode(value: &str) -> Result<RuntimeMode, BackendError> {
    match value {
        "portable" => Ok(RuntimeMode::Portable),
        "studio-run-locally" => Ok(RuntimeMode::StudioRunLocally),
        "external-url" => Ok(RuntimeMode::ExternalUrl),
        _ => Err(BackendError::invalid_request(
            "unknown Runtime mode; expected portable or studio-run-locally",
        )),
    }
}

fn execution_context(requested: Option<BackendId>) -> Result<ExecutionContext, BackendError> {
    let snapshot = crate::platform::capability_snapshot(requested)?;
    let session = SessionDescriptor::create(snapshot.clone())?;
    Ok(ExecutionContext { snapshot, session })
}

fn success_execution(
    command: &str,
    format: OutputFormat,
    context: &ExecutionContext,
    output: CommandOutput,
    include_snapshot: bool,
) -> CliExecution {
    let envelope = SuccessEnvelope {
        schema_version: CONTRACT_SCHEMA_VERSION,
        command,
        ok: true,
        platform: context.snapshot.manifest.host_platform,
        backend: context.snapshot.manifest.backend,
        session_id: &context.session.session_id,
        capability_snapshot: include_snapshot.then_some(&context.snapshot),
        operation_id: output.operation_id.as_deref(),
        studio_session_id: output.studio_session_id.as_deref(),
        runtime_session_id: output.runtime_session_id.as_deref(),
        data: &output.data,
    };
    let mut stdout = String::new();
    if format == OutputFormat::Ndjson {
        for progress in &output.progress {
            stdout.push_str(&json_line(&ProgressEnvelope {
                schema_version: CONTRACT_SCHEMA_VERSION,
                command,
                event: "progress",
                session_id: &context.session.session_id,
                progress,
            }));
        }
    }
    stdout.push_str(&json_line(&envelope));
    let exit_code = if command == "browser.test"
        && output.data.get("outcome").and_then(Value::as_str) == Some("failed")
    {
        EXIT_OPERATION_FAILED
    } else {
        EXIT_OK
    };
    CliExecution {
        exit_code,
        stdout,
        stderr: String::new(),
    }
}

fn error_execution(
    command: &str,
    _format: OutputFormat,
    context: &ExecutionContext,
    error: BackendError,
    include_snapshot: bool,
) -> CliExecution {
    let exit_code = exit_code(&error);
    let envelope = ErrorEnvelope {
        schema_version: CONTRACT_SCHEMA_VERSION,
        command,
        ok: false,
        platform: context.snapshot.manifest.host_platform,
        backend: context.snapshot.manifest.backend,
        session_id: &context.session.session_id,
        capability_snapshot: include_snapshot.then_some(&context.snapshot),
        error: &error,
    };
    CliExecution {
        exit_code,
        stdout: String::new(),
        stderr: json_line(&envelope),
    }
}

fn context_error_execution(
    command: &str,
    format: OutputFormat,
    error: BackendError,
) -> CliExecution {
    match execution_context(None) {
        Ok(context) => error_execution(
            command,
            format,
            &context,
            sanitize_backend_error(error),
            false,
        ),
        Err(_) => bare_error_execution(command, sanitize_backend_error(error)),
    }
}

fn bootstrap_error_execution(arguments: &[OsString], error: BackendError) -> CliExecution {
    let command = bootstrap_command_name(arguments);
    context_error_execution(command, OutputFormat::Json, error)
}

fn bare_error_execution(command: &str, error: BackendError) -> CliExecution {
    let exit_code = exit_code(&error);
    let value = json!({
        "schemaVersion": CONTRACT_SCHEMA_VERSION,
        "command": command,
        "ok": false,
        "platform": PlatformId::current(),
        "backend": expected_backend(),
        "sessionId": "session_unavailable",
        "capabilitySnapshot": null,
        "error": error,
    });
    CliExecution {
        exit_code,
        stdout: String::new(),
        stderr: json_line(&value),
    }
}

fn command_error_to_backend(error: CommandError, backend: BackendId) -> BackendError {
    if let Some(details) = error.details {
        return sanitize_backend_error(*details);
    }
    let code = match error.code {
        CommandErrorCode::UnsupportedCapability => BackendErrorCode::UnsupportedCapability,
        CommandErrorCode::BackendMismatch => BackendErrorCode::BackendMismatch,
        CommandErrorCode::InvalidRequest => BackendErrorCode::InvalidRequest,
        CommandErrorCode::PreconditionFailed => BackendErrorCode::PreconditionFailed,
        CommandErrorCode::ExternalProcessTimeout => BackendErrorCode::ExternalProcessTimeout,
        CommandErrorCode::ExternalProcessCancelled => BackendErrorCode::ExternalProcessCancelled,
        CommandErrorCode::ExternalProcessInterrupted => {
            BackendErrorCode::ExternalProcessInterrupted
        }
        CommandErrorCode::ToolchainUnavailable => BackendErrorCode::ToolchainUnavailable,
        CommandErrorCode::RuntimeVersionUnsupported => BackendErrorCode::RuntimeVersionUnsupported,
        CommandErrorCode::ConsistencyFailed => BackendErrorCode::ConsistencyFailed,
        CommandErrorCode::RuntimeBuildFailed => BackendErrorCode::RuntimeBuildFailed,
        CommandErrorCode::RuntimeInitializationFailed => {
            BackendErrorCode::RuntimeInitializationFailed
        }
        CommandErrorCode::RuntimeReadinessTimeout => BackendErrorCode::RuntimeReadinessTimeout,
        CommandErrorCode::RuntimeSessionNotFound => BackendErrorCode::RuntimeSessionNotFound,
        CommandErrorCode::RuntimeExited => BackendErrorCode::RuntimeExited,
        CommandErrorCode::RuntimeGuestOffline => BackendErrorCode::RuntimeGuestOffline,
        CommandErrorCode::RuntimePortConflict => BackendErrorCode::RuntimePortConflict,
        CommandErrorCode::RuntimePortForwardingInvalid => {
            BackendErrorCode::RuntimePortForwardingInvalid
        }
        CommandErrorCode::RuntimeFirewallBlocked => BackendErrorCode::RuntimeFirewallBlocked,
        CommandErrorCode::RuntimeNotListening => BackendErrorCode::RuntimeNotListening,
        CommandErrorCode::RuntimeComposeRecoveryFailed => {
            BackendErrorCode::RuntimeComposeRecoveryFailed
        }
        CommandErrorCode::ComposeNotWinboat | CommandErrorCode::ComposeAmbiguous => {
            BackendErrorCode::InvalidRequest
        }
        CommandErrorCode::ComposeRevisionConflict => BackendErrorCode::ConsistencyFailed,
        CommandErrorCode::ConfigLoadFailed
        | CommandErrorCode::DownloadCancelled
        | CommandErrorCode::InstallFailed
        | CommandErrorCode::OperationFailed => BackendErrorCode::OperationFailed,
    };
    BackendError {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        code,
        message: safe_error_message_for_backend(code, Some(backend)).to_string(),
        backend: Some(backend),
        capability: None,
        reason: None,
        retryable: matches!(
            error.code,
            CommandErrorCode::DownloadCancelled
                | CommandErrorCode::InstallFailed
                | CommandErrorCode::OperationFailed
                | CommandErrorCode::ExternalProcessTimeout
                | CommandErrorCode::ExternalProcessCancelled
                | CommandErrorCode::ExternalProcessInterrupted
                | CommandErrorCode::ComposeRevisionConflict
        ),
        diagnostic_ref: None,
    }
}

fn sanitize_backend_error(error: BackendError) -> BackendError {
    let diagnostic_ref = error
        .diagnostic_ref
        .filter(|value| is_safe_artifact_reference(value));
    BackendError {
        schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
        code: error.code,
        message: safe_error_message_for_backend(error.code, error.backend).to_string(),
        backend: error.backend,
        capability: error.capability,
        reason: None,
        retryable: error.retryable,
        diagnostic_ref,
    }
}

fn safe_error_message(code: BackendErrorCode) -> &'static str {
    match code {
        BackendErrorCode::UnsupportedCapability => {
            "the selected backend does not support this command"
        }
        BackendErrorCode::BackendMismatch => "the selected backend does not match this host",
        BackendErrorCode::InvalidRequest => "the command request is invalid",
        BackendErrorCode::PreconditionFailed => "a required precondition was not satisfied",
        BackendErrorCode::OperationFailed => "the command could not be completed",
        BackendErrorCode::ExternalProcessTimeout => {
            "an external process did not finish before its deadline"
        }
        BackendErrorCode::ExternalProcessCancelled => "an external process was cancelled",
        BackendErrorCode::ExternalProcessInterrupted => {
            "an external process was interrupted during cleanup"
        }
        BackendErrorCode::ToolchainUnavailable => {
            "the exact-version runtime toolchain is unavailable"
        }
        BackendErrorCode::RuntimeVersionUnsupported => {
            "the exact project version requires Windows Studio Pro Run Locally"
        }
        BackendErrorCode::ConsistencyFailed => "the Mendix project has consistency errors",
        BackendErrorCode::RuntimeBuildFailed => "the Portable Runtime package build failed",
        BackendErrorCode::RuntimeInitializationFailed => {
            "the Portable Runtime failed during initialization"
        }
        BackendErrorCode::RuntimeReadinessTimeout => {
            "the Portable Runtime did not become HTTP-ready before the timeout"
        }
        BackendErrorCode::RuntimeSessionNotFound => "the Portable Runtime session was not found",
        BackendErrorCode::RuntimeExited => "the Portable Runtime exited unexpectedly",
        BackendErrorCode::RuntimeGuestOffline => "the WinBoat guest is offline",
        BackendErrorCode::RuntimePortConflict => "the WinBoat Runtime host port conflicts",
        BackendErrorCode::RuntimePortForwardingInvalid => {
            "the WinBoat Runtime port forwarding is invalid"
        }
        BackendErrorCode::RuntimeFirewallBlocked => {
            "the Windows firewall blocks the WinBoat Runtime port"
        }
        BackendErrorCode::RuntimeNotListening => {
            "the Mendix Runtime is not listening inside the WinBoat guest"
        }
        BackendErrorCode::RuntimeComposeRecoveryFailed => {
            "the original WinBoat Compose configuration could not be recovered"
        }
    }
}

fn safe_error_message_for_backend(
    code: BackendErrorCode,
    backend: Option<BackendId>,
) -> &'static str {
    if code == BackendErrorCode::RuntimeSessionNotFound && backend == Some(BackendId::LinuxWinboat)
    {
        return "the WinBoat Runtime session was not found";
    }
    safe_error_message(code)
}

fn exit_code(error: &BackendError) -> i32 {
    match error.code {
        BackendErrorCode::InvalidRequest => EXIT_INVALID_REQUEST,
        BackendErrorCode::BackendMismatch
        | BackendErrorCode::UnsupportedCapability
        | BackendErrorCode::RuntimeVersionUnsupported => EXIT_BACKEND_UNAVAILABLE,
        BackendErrorCode::PreconditionFailed
        | BackendErrorCode::OperationFailed
        | BackendErrorCode::ExternalProcessTimeout
        | BackendErrorCode::ExternalProcessCancelled
        | BackendErrorCode::ExternalProcessInterrupted
        | BackendErrorCode::ToolchainUnavailable
        | BackendErrorCode::ConsistencyFailed
        | BackendErrorCode::RuntimeBuildFailed
        | BackendErrorCode::RuntimeInitializationFailed
        | BackendErrorCode::RuntimeReadinessTimeout
        | BackendErrorCode::RuntimeSessionNotFound
        | BackendErrorCode::RuntimeExited
        | BackendErrorCode::RuntimeGuestOffline
        | BackendErrorCode::RuntimePortConflict
        | BackendErrorCode::RuntimePortForwardingInvalid
        | BackendErrorCode::RuntimeFirewallBlocked
        | BackendErrorCode::RuntimeNotListening
        | BackendErrorCode::RuntimeComposeRecoveryFailed => EXIT_OPERATION_FAILED,
    }
}

fn is_safe_artifact_reference(value: &str) -> bool {
    value.strip_prefix("artifact_").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn json_line<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| {
        format!(
            "{{\"schemaVersion\":\"{CONTRACT_SCHEMA_VERSION}\",\"command\":\"unknown\",\"ok\":false}}"
        )
    });
    format!("{json}\n")
}

fn is_headless_command(argument: Option<&OsString>) -> bool {
    matches!(
        argument.and_then(|value| value.to_str()),
        Some("capabilities" | "env" | "studio" | "runtime" | "browser" | "project" | "operation")
    )
}

fn bootstrap_command_name(arguments: &[OsString]) -> &'static str {
    match arguments.first().and_then(|value| value.to_str()) {
        Some("capabilities") => "capabilities",
        Some("env") => "env",
        Some("studio") => "studio",
        Some("runtime") => "runtime",
        Some("browser") => "browser",
        Some("project") => "project",
        Some("operation") => "operation",
        _ => "unknown",
    }
}

fn expected_backend() -> BackendId {
    crate::platform::backend::expected_backend(PlatformId::current())
        .unwrap_or(BackendId::MacNative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::CapabilityId;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn app_config(workspace: &std::path::Path) -> crate::models::AppConfig {
        crate::models::AppConfig {
            language_preference: "en-US".into(),
            winboat_setup_pending: false,
            winboat_executable: "missing-winboat".into(),
            compose_file: "missing-compose.yml".into(),
            container_runtime: crate::models::ContainerRuntime::Docker,
            container_name: "WinBoat".into(),
            api_url: "http://127.0.0.1:9".into(),
            rdp_host: "127.0.0.1".into(),
            rdp_port: 9,
            shared_directory: workspace.to_string_lossy().into_owned(),
            windows_shared_directory: r"\\host.lan\Data".into(),
            freerdp_binary: "missing-freerdp".into(),
            mendix_install_root: r"C:\Program Files\Mendix".into(),
            mendix_data_root: r"C:\ProgramData\Mendix".into(),
            windows_studio_paths: Vec::new(),
            startup_timeout_seconds: 1,
        }
    }

    #[test]
    fn help_covers_the_complete_command_tree_without_backend_output() {
        let command_tree = [
            vec!["capabilities"],
            vec!["env"],
            vec!["env", "status"],
            vec!["env", "ensure"],
            vec!["studio"],
            vec!["studio", "list"],
            vec!["studio", "install"],
            vec!["studio", "uninstall"],
            vec!["studio", "start"],
            vec!["studio", "status"],
            vec!["studio", "stop"],
            vec!["runtime"],
            vec!["runtime", "build"],
            vec!["runtime", "start"],
            vec!["runtime", "list"],
            vec!["runtime", "status"],
            vec!["runtime", "wait"],
            vec!["runtime", "url"],
            vec!["runtime", "stop"],
            vec!["runtime", "forget"],
            vec!["runtime", "logs"],
            vec!["browser"],
            vec!["browser", "doctor"],
            vec!["browser", "install"],
            vec!["browser", "test"],
            vec!["browser", "artifacts"],
            vec!["project"],
            vec!["project", "list"],
            vec!["project", "version"],
            vec!["operation"],
            vec!["operation", "list"],
            vec!["operation", "status"],
            vec!["operation", "retry"],
        ];
        for command in command_tree {
            let mut arguments = args(&command);
            arguments.push("--help".into());
            let execution = execute(&arguments).expect("command help execution");
            assert_eq!(execution.exit_code, 0);
            assert!(execution.stderr.is_empty());
            assert!(
                execution
                    .stdout
                    .contains(&format!("Usage: mendimaru {}", command.join(" "))),
                "missing usage for {}",
                command.join(" ")
            );
            assert!(!execution.stdout.contains("schemaVersion"));
            assert!(!execution.stdout.contains("snapshotId"));
        }
    }

    #[test]
    fn sanitized_cli_errors_preserve_cause_codes_and_stable_existing_contracts() {
        let mut cause = crate::contracts::BackendError::operation(
            BackendId::LinuxWinboat,
            CapabilityId::StudioStart,
            "the WinBoat Runtime session was not found",
        );
        cause.code = crate::contracts::BackendErrorCode::RuntimeSessionNotFound;
        cause.retryable = false;
        let sanitized = command_error_to_backend(
            crate::models::CommandError::from(cause),
            BackendId::LinuxWinboat,
        );
        assert_eq!(
            sanitized.code,
            crate::contracts::BackendErrorCode::RuntimeSessionNotFound
        );
        assert_eq!(
            sanitized.message,
            "the WinBoat Runtime session was not found"
        );
        assert!(!sanitized.retryable);
        assert_eq!(exit_code(&sanitized), EXIT_OPERATION_FAILED);

        let portable = command_error_to_backend(
            crate::models::CommandError::new(
                crate::models::CommandErrorCode::RuntimeSessionNotFound,
                "fixture".to_string(),
            ),
            BackendId::WindowsNative,
        );
        assert_eq!(
            portable.message,
            "the Portable Runtime session was not found"
        );

        let operation = command_error_to_backend(
            crate::models::CommandError::new(
                crate::models::CommandErrorCode::OperationFailed,
                "fixture".to_string(),
            ),
            BackendId::LinuxWinboat,
        );
        assert_eq!(
            operation.code,
            crate::contracts::BackendErrorCode::OperationFailed
        );
        assert_eq!(operation.message, "the command could not be completed");
        assert!(operation.retryable);
        assert_eq!(exit_code(&operation), EXIT_OPERATION_FAILED);
    }

    #[test]
    fn global_flags_can_precede_subcommand_help() {
        let execution =
            execute(&args(&["--json", "env", "status", "--help"])).expect("help after flags");
        assert_eq!(execution.exit_code, 0);
        assert!(execution.stdout.contains("Usage: mendimaru env status"));
        assert!(execution.stderr.is_empty());
    }

    #[test]
    fn orphan_status_keeps_only_sessions_outside_the_trusted_keepers() {
        fn session(id: &str) -> crate::contracts::StudioSessionStatus {
            serde_json::from_value(json!({
                "schemaVersion": CONTRACT_SCHEMA_VERSION,
                "sessionId": id,
                "version": "11.12.2",
                "state": "running",
                "processId": 4242,
                "connection": "disconnected",
                "reconnectable": false
            }))
            .expect("Studio session fixture")
        }
        let owned = session("studio-4242-638908128000000000");
        let orphan = session("studio-9000-638908128000000000");

        let orphans = orphan_studio_sessions(vec![owned.clone(), orphan.clone()], &[owned]);
        assert_eq!(orphans, vec![orphan]);
    }

    #[test]
    fn a_failed_stop_becomes_success_only_after_authoritative_absence() {
        let stopped_session = serde_json::from_value(json!({
            "schemaVersion": CONTRACT_SCHEMA_VERSION,
            "sessionId": "studio-4242-638908128000000000",
            "version": "11.12.2",
            "state": "running",
            "processId": 4242,
            "connection": "connected",
            "reconnectable": false
        }))
        .expect("Studio session fixture");
        let other_session = serde_json::from_value(json!({
            "schemaVersion": CONTRACT_SCHEMA_VERSION,
            "sessionId": "studio-9000-638908128000000000",
            "version": "11.12.2",
            "state": "running",
            "processId": 9000,
            "connection": "connected",
            "reconnectable": false
        }))
        .expect("other Studio session fixture");
        let session_id = "studio-4242-638908128000000000";
        let cleanup_root = tempfile::tempdir().expect("temporary cleanup workspace");
        let cleanup_config = app_config(cleanup_root.path());

        complete_stopped_session_if_absent(
            CommandError::new(CommandErrorCode::OperationFailed, "fixture failure".into()),
            Ok(vec![other_session]),
            &cleanup_config,
            session_id,
        )
        .expect("an absent target is idempotently complete");

        let retained = complete_stopped_session_if_absent(
            CommandError::new(CommandErrorCode::OperationFailed, "fixture failure".into()),
            Ok(vec![stopped_session]),
            &cleanup_config,
            session_id,
        )
        .expect_err("a live target remains a failure");
        assert_eq!(retained.message, "fixture failure");

        complete_stopped_session_if_absent(
            CommandError::new(CommandErrorCode::OperationFailed, "fixture failure".into()),
            Err(CommandError::new(
                CommandErrorCode::OperationFailed,
                "status failed".into(),
            )),
            &cleanup_config,
            session_id,
        )
        .expect_err("an unverifiable target remains a failure");

        complete_stopped_session_if_absent(
            CommandError::new(CommandErrorCode::InvalidRequest, "invalid session".into()),
            Ok(Vec::new()),
            &cleanup_config,
            session_id,
        )
        .expect_err("an invalid request is never converted to stop success");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn an_unconfirmed_keeper_stop_remains_fallback_eligible() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let root = tempfile::tempdir().expect("temporary app root");
        let directory = root.path().join("cache").join("cli-sessions");
        std::fs::create_dir_all(&directory).expect("session socket directory");
        let session_id = "studio-4242-638908128000000000";
        let socket_path = directory.join(session_socket_name(session_id));
        let listener = UnixListener::bind(&socket_path).expect("fixture keeper socket");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("keeper client connects");
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            reader.read_line(&mut request).expect("read keeper request");
            let mut stream = reader.into_inner();
            stream
                .write_all(b"{\"ok\":false,\"session\":null}\n")
                .expect("write unconfirmed keeper response");
        });
        let paths = AppPaths::for_tests(root.path().join("config"), root.path().join("cache"));

        let stopped = request_keeper_stop(&paths, session_id)
            .await
            .expect("an unconfirmed keeper response is not a CLI transport error");
        assert!(!stopped);
        server.join().expect("fixture keeper server completes");
    }

    #[test]
    fn ordinary_responses_omit_the_snapshot_until_requested() {
        let snapshot = CapabilitySnapshot::capture(crate::platform::backend::manifest_for(
            BackendId::LinuxWinboat,
            "fixture-architecture",
        ))
        .expect("fake capability snapshot");
        let context = ExecutionContext {
            session: SessionDescriptor::create(snapshot.clone()).expect("fake session"),
            snapshot,
        };
        let output =
            || CommandOutput::data(json!({ "ready": true })).expect("fixture command output");
        let omitted =
            success_execution("env.status", OutputFormat::Json, &context, output(), false);
        let document: Value = serde_json::from_str(&omitted.stdout).expect("lightweight JSON");
        assert!(document.get("capabilitySnapshot").is_none());
        assert!(omitted.stdout.len() < 300);

        let included =
            success_execution("env.status", OutputFormat::Json, &context, output(), true);
        let document: Value = serde_json::from_str(&included.stdout).expect("snapshot JSON");
        assert!(document["capabilitySnapshot"].is_object());
    }

    #[test]
    fn unrelated_arguments_continue_to_the_desktop_app() {
        assert_eq!(execute(&[]), None);
        assert!(execute(&args(&["project.mpr"])).is_some());
    }

    #[test]
    fn capabilities_returns_one_complete_json_envelope_without_tauri_initialization() {
        let execution =
            execute(&args(&["capabilities", "--json"])).expect("recognized CLI command");
        assert_eq!(execution.exit_code, EXIT_OK);
        assert!(execution.stderr.is_empty());
        assert_eq!(execution.stdout.lines().count(), 1);
        let json: Value = serde_json::from_str(&execution.stdout).expect("stdout JSON");
        assert_eq!(json["schemaVersion"], CONTRACT_SCHEMA_VERSION);
        assert_eq!(json["command"], "capabilities");
        assert_eq!(json["ok"], true);
        assert_eq!(
            json["platform"],
            serde_json::to_value(PlatformId::current()).unwrap()
        );
        assert_eq!(json["data"], json["capabilitySnapshot"]);
        assert!(json["sessionId"]
            .as_str()
            .is_some_and(|id| id.starts_with("session_")));
        assert_eq!(
            json["data"]["manifest"]["capabilities"]
                .as_array()
                .expect("capability list")
                .len(),
            CapabilityId::ALL.len()
        );
    }

    #[test]
    fn mismatched_backend_is_a_structured_stderr_error_without_fallback() {
        let mismatched = match PlatformId::current() {
            PlatformId::Linux => "windows-native",
            PlatformId::Windows => "mac-native",
            PlatformId::Macos | PlatformId::Unsupported => "linux-winboat",
        };
        let execution = execute(&args(&[
            "capabilities",
            "--json",
            "--snapshot",
            "--backend",
            mismatched,
        ]))
        .expect("recognized CLI command");
        assert_eq!(execution.exit_code, EXIT_BACKEND_UNAVAILABLE);
        assert!(execution.stdout.is_empty());
        let json: Value = serde_json::from_str(&execution.stderr).expect("stderr JSON");
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "backend_mismatch");
        assert_ne!(json["backend"], mismatched);
        assert!(json.get("capabilitySnapshot").is_none());
    }

    #[test]
    fn parser_accepts_the_complete_command_surface_and_rejects_ambiguity() {
        let valid = [
            args(&["env", "status", "--json"]),
            args(&["env", "ensure", "--timeout-seconds", "30"]),
            args(&["studio", "list"]),
            args(&["studio", "status", "--refresh"]),
            args(&["studio", "status", "--orphans"]),
            args(&["studio", "install", "--version", "11.12.2", "--ndjson"]),
            args(&["studio", "uninstall", "--version=11.12.2"]),
            args(&[
                "studio",
                "start",
                "--version",
                "11.12.2",
                "--project-id",
                &format!("project_{}", "a".repeat(64)),
            ]),
            args(&["studio", "status"]),
            args(&[
                "studio",
                "stop",
                "--session-id",
                "studio-1-700000000000000000",
            ]),
            args(&["runtime", "list"]),
            args(&[
                "runtime",
                "forget",
                "--session-id",
                &format!("runtime_{}", "e".repeat(32)),
            ]),
            args(&["project", "list"]),
            args(&[
                "project",
                "version",
                "--project-id",
                &format!("project_{}", "b".repeat(64)),
            ]),
            args(&["operation", "list"]),
            args(&["operation", "status", "--operation-id", "install-11.12.2-a"]),
            args(&["operation", "retry", "--operation-id", "install-11.12.2-a"]),
            args(&["browser", "doctor"]),
            args(&["browser", "install", "chromium"]),
            args(&[
                "browser",
                "test",
                "--base-url",
                "http://127.0.0.1:8080",
                "--suite-path",
                "tests/browser/smoke.browser.json",
                "--fail-on-console-error",
                "--fail-on-network-failure",
                "--record-har",
                "--max-artifact-mib",
                "64",
                "--retention-runs",
                "5",
            ]),
            args(&[
                "browser",
                "artifacts",
                "--session-id",
                &format!("session_{}", "c".repeat(32)),
            ]),
        ];
        for arguments in valid {
            parse(&arguments).expect("valid CLI shape");
        }

        for arguments in [
            args(&[
                "studio",
                "install",
                "--version",
                "11.12.2",
                "--version",
                "11.13.0",
            ]),
            args(&["studio", "start", "--project-path", "/secret/app.mpr"]),
            args(&["project", "version"]),
            args(&["env", "ensure", "--json", "--ndjson"]),
            args(&["operation", "cancel", "--operation-id", "x"]),
            args(&["runtime", "list", "--refresh"]),
            args(&["studio", "list", "--timeout-seconds", "0"]),
            args(&[
                "browser",
                "test",
                "--base-url",
                "http://127.0.0.1:8080",
                "--runtime-session-id",
                &format!("runtime_{}", "d".repeat(32)),
                "--suite-path",
                "suite.json",
            ]),
            args(&[
                "browser",
                "test",
                "--suite-path",
                "suite.json",
                "--navigation-timeout-ms",
                "99",
            ]),
        ] {
            let error = parse(&arguments).expect_err("invalid CLI shape");
            assert_eq!(error.code, BackendErrorCode::InvalidRequest);
        }
    }

    #[test]
    fn parse_errors_never_echo_unknown_arguments_or_secret_values() {
        let secret = "super-secret-password";
        let execution = execute(&args(&["studio", "list", "--password", secret]))
            .expect("recognized CLI command");
        assert_eq!(execution.exit_code, EXIT_INVALID_REQUEST);
        assert!(execution.stdout.is_empty());
        assert!(!execution.stderr.contains(secret));
        assert!(!execution.stderr.contains("--password"));
        let json: Value = serde_json::from_str(&execution.stderr).expect("stderr JSON");
        assert_eq!(json["error"]["code"], "invalid_request");
    }

    #[test]
    fn the_same_cli_fixture_has_the_same_shape_for_every_fake_platform_manifest() {
        use std::collections::BTreeSet;

        let mut expected_keys = None;
        for backend in BackendId::ALL {
            let snapshot = CapabilitySnapshot::capture(crate::platform::backend::manifest_for(
                backend,
                "fixture-architecture",
            ))
            .expect("fake capability snapshot");
            let session = SessionDescriptor::create(snapshot.clone()).expect("fake session");
            let context = ExecutionContext { snapshot, session };
            let execution = success_execution(
                "env.status",
                OutputFormat::Json,
                &context,
                CommandOutput::data(json!({
                    "ready": false,
                    "containerStatus": "not-found",
                    "checks": [],
                }))
                .expect("fixture output"),
                false,
            );
            let document: Value = serde_json::from_str(&execution.stdout).expect("CLI JSON");
            let keys = document
                .as_object()
                .expect("response object")
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>();
            if let Some(expected) = &expected_keys {
                assert_eq!(&keys, expected);
            } else {
                expected_keys = Some(keys);
            }
            assert_eq!(document["backend"], backend.as_str());
            assert_eq!(
                document["platform"],
                serde_json::to_value(context.snapshot.manifest.host_platform)
                    .expect("platform serializes")
            );
            assert_eq!(document["command"], "env.status");
            assert_eq!(document["data"]["ready"], false);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn session_keeper_socket_contract_is_private_bounded_and_symlink_safe() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temporary = tempfile::tempdir().expect("temporary directory");
        let paths = AppPaths::for_tests(
            temporary.path().join("config"),
            temporary.path().join("cache"),
        );
        let directory = ensure_session_socket_directory(&paths).expect("keeper directory");
        let mode = std::fs::symlink_metadata(&directory)
            .expect("keeper metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);

        let name = session_socket_name("studio-4242-638908128000000000");
        assert!(name.starts_with("s-"));
        assert!(name.ends_with(".sock"));
        assert_eq!(name.len(), 39);
        assert!(!name.contains("4242"));

        let other = tempfile::tempdir().expect("other temporary directory");
        let unsafe_paths =
            AppPaths::for_tests(other.path().join("config"), other.path().join("cache"));
        unsafe_paths
            .ensure_cache_directory()
            .expect("unsafe cache directory");
        let target = other.path().join("target");
        std::fs::create_dir(&target).expect("target directory");
        symlink(&target, unsafe_paths.cache_directory().join("cli-sessions"))
            .expect("directory symlink");
        assert!(ensure_session_socket_directory(&unsafe_paths).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_keeper_sockets_are_cleaned_without_touching_live_or_untrusted_entries() {
        use std::io::Write as _;
        use std::os::unix::fs::{symlink, FileTypeExt, PermissionsExt};
        use std::os::unix::net::UnixListener;

        let temporary = tempfile::tempdir().expect("temporary socket directory");
        let directory = temporary.path().join("cli-sessions");
        std::fs::create_dir_all(&directory).expect("socket directory");
        let live_path = directory.join(format!("s-{}.sock", "1".repeat(32)));
        let stale_path = directory.join(format!("s-{}.sock", "2".repeat(32)));
        let preserve_path = directory.join(format!("s-{}.sock", "3".repeat(32)));
        let live_listener = UnixListener::bind(&live_path).expect("live keeper socket");
        std::fs::set_permissions(&live_path, std::fs::Permissions::from_mode(0o600))
            .expect("live socket permissions");
        let stale_listener = UnixListener::bind(&stale_path).expect("temporary stale socket");
        std::fs::set_permissions(&stale_path, std::fs::Permissions::from_mode(0o600))
            .expect("stale socket permissions");
        drop(stale_listener);

        let regular = directory.join("audit.txt");
        let mut file = std::fs::File::create(&regular).expect("regular fixture");
        file.write_all(b"unrelated")
            .expect("regular fixture content");
        let link = directory.join("s-linked.sock");
        symlink(&regular, &link).expect("socket-like symlink");

        let removed = cleanup_stale_session_keeper_sockets(&directory, &preserve_path)
            .expect("stale keeper socket cleanup");
        assert_eq!(removed, 1);
        assert!(std::fs::symlink_metadata(&live_path)
            .expect("live keeper remains")
            .file_type()
            .is_socket());
        assert!(!stale_path.exists());
        assert!(regular.is_file());
        assert!(std::fs::symlink_metadata(&link)
            .expect("symlink remains")
            .file_type()
            .is_symlink());
        drop(live_listener);

        let audit_path = directory.join("socket-cleanup.log");
        let audit = std::fs::read_to_string(&audit_path).expect("socket cleanup audit");
        let report: serde_json::Value =
            serde_json::from_str(audit.lines().next().expect("audit record")).expect("audit JSON");
        assert_eq!(report["removedSockets"], 1);
        assert_eq!(report["retainedLiveSockets"], 1);
        assert!(!audit.contains(temporary.path().to_string_lossy().as_ref()));
        assert_eq!(
            std::fs::metadata(&audit_path)
                .expect("audit metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn session_keeper_ipc_accepts_only_bounded_json_from_a_private_socket() {
        use std::io::{BufRead, Write};
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("temporary directory");
        let socket_path = temporary.path().join("keeper.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&socket_path).expect("fixture listener");
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
            .expect("fixture permissions");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("fixture connection");
            let mut request = String::new();
            std::io::BufReader::new(stream.try_clone().expect("fixture reader"))
                .read_line(&mut request)
                .expect("fixture request");
            assert_eq!(request, "status\n");
            let response = json!({
                "ok": true,
                "session": {
                    "schemaVersion": CONTRACT_SCHEMA_VERSION,
                    "sessionId": "studio-4242-638908128000000000",
                    "version": "11.12.2",
                    "state": "running",
                    "processId": 4242,
                    "startedAt": "2026-08-15T00:00:00Z",
                    "projectName": "Orders",
                    "connection": "connected",
                    "reconnectable": false,
                    "reconnectUnavailable": "already-connected"
                }
            });
            let mut stream = stream;
            writeln!(stream, "{response}").expect("fixture response");
        });
        let response =
            tauri::async_runtime::block_on(request_session_keeper(&socket_path, "status"))
                .expect("keeper request")
                .expect("keeper response");
        assert!(response.ok);
        assert_eq!(
            response.session.expect("session").session_id,
            "studio-4242-638908128000000000"
        );
        server.join().expect("fixture server");

        let untrusted = temporary.path().join("regular-file.sock");
        std::fs::write(&untrusted, b"not a socket").expect("untrusted fixture");
        let result = tauri::async_runtime::block_on(request_session_keeper(&untrusted, "status"));
        assert!(result.is_err());
    }
}
