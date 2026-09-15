//! Host-side diagnostics must work even when the JavaScript entry point cannot load.
use super::*;
use crate::process::{self, CommandFailureKind, CommandPolicy};
use std::error::Error;

const CAPTURE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Cause {
    RunnerMissing,
    RunnerUnreadable,
    UnsafeOverride,
    NodeMissing,
    NodeSpawnDenied,
    NodeSpawnFailed,
    NodeUnsupported,
    NodeProbeFailed,
    ProbeTimeout,
    JsDependenciesMissing,
    RunnerFailed,
    RunnerOutputInvalid,
    ChromiumUnavailable,
}

impl Cause {
    fn guidance(self) -> (&'static str, &'static str) {
        match self {
            Self::RunnerMissing => ("The browser runner is missing.", "Reinstall Mendimaru with its browser resources, or correct MENDIMARU_BROWSER_RUNNER_PATH."),
            Self::RunnerUnreadable => ("The browser runner cannot be read.", "Restore read permission on the installed browser resources."),
            Self::UnsafeOverride => ("A browser tool override is unsafe.", "Unset the override for this check, or use an absolute path to a direct regular file; symlinks and directories are rejected."),
            Self::NodeMissing => ("Node.js is missing.", "Install Node.js 22.22.2 or later and make node available on PATH, or correct MENDIMARU_NODE_BINARY."),
            Self::NodeSpawnDenied => ("Starting Node.js was denied.", "Restore Node.js executable permissions and check execution restrictions on its filesystem."),
            Self::NodeSpawnFailed => ("Node.js could not start.", "Reinstall a Node.js executable compatible with this host."),
            Self::NodeUnsupported => ("The Node.js version is unsupported.", "Upgrade Node.js to 22.22.2 or later, including any MENDIMARU_NODE_BINARY override."),
            Self::NodeProbeFailed => ("Node.js did not return a valid version.", "Check that node or MENDIMARU_NODE_BINARY runs a working Node.js executable."),
            Self::ProbeTimeout => ("A browser prerequisite check timed out.", "Check that Node.js and Chromium can start, then rerun browser doctor."),
            Self::JsDependenciesMissing => ("Browser JavaScript dependencies could not be resolved.", "Reinstall Mendimaru's locked browser dependencies; in a source checkout run npm ci."),
            Self::RunnerFailed => ("The browser runner failed while loading or executing.", "Inspect the private browser doctor diagnostic and reinstall matching runner resources and dependencies."),
            Self::RunnerOutputInvalid => ("The browser runner returned an invalid diagnostic response.", "Reinstall matching Mendimaru browser resources and remove stale runner overrides."),
            Self::ChromiumUnavailable => ("The pinned Chromium build is missing or cannot launch.", "Run mendimaru browser install chromium, then check Playwright's browser system dependencies if launching still fails."),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PrerequisiteCheck {
    id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<Cause>,
    message: String,
    action: String,
}

impl PrerequisiteCheck {
    fn skipped(id: &str) -> Self {
        Self {
            id: id.into(),
            status: "skipped".into(),
            code: None,
            message: "This prerequisite could not be checked.".into(),
            action: "Resolve the failed checks and rerun browser doctor.".into(),
        }
    }

    fn set(&mut self, cause: Option<Cause>) {
        self.code = cause;
        let (message, action) = if let Some(cause) = cause {
            self.status = "failed".into();
            cause.guidance()
        } else {
            self.status = "passed".into();
            ("This prerequisite is available.", "No action required.")
        };
        self.message = message.into();
        self.action = action.into();
    }
}

/// Only fixed, allowlisted tokens survive capture. Paths, arbitrary module names,
/// source excerpts, stack frames and environment values are never persisted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DoctorDiagnostic {
    reference: String,
    stored: bool,
    stderr_truncated: bool,
    stdout_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module: Option<String>,
}

impl DoctorDiagnostic {
    fn capture(&mut self, output: &process::CommandOutput) {
        self.stderr_truncated |= output.stderr_truncated;
        self.stdout_truncated |= output.stdout_truncated;
        let stderr = String::from_utf8_lossy(&output.stderr);
        self.error_kind = [
            "ERR_MODULE_NOT_FOUND",
            "MODULE_NOT_FOUND",
            "ERR_PACKAGE_PATH_NOT_EXPORTED",
            "ERR_DLOPEN_FAILED",
            "SyntaxError",
            "TypeError",
            "ReferenceError",
        ]
        .into_iter()
        .find(|kind| stderr.contains(kind))
        .map(str::to_string);
        self.module = [
            "@playwright/test",
            "playwright-core",
            "playwright",
            "fflate",
            "browser-artifact-safety.mjs",
        ]
        .into_iter()
        .find(|module| stderr.contains(module))
        .map(str::to_string);
    }
}

pub(super) async fn inspect(backend: BackendId) -> BrowserDoctor {
    let mut report = BrowserDoctor {
        schema_version: CONTRACT_SCHEMA_VERSION.into(),
        runner_version: RUNNER_VERSION.into(),
        ready: false,
        node_version: "unavailable".into(),
        minimum_node_version: MINIMUM_NODE_VERSION.into(),
        node_supported: false,
        playwright_version: "unavailable".into(),
        chromium: ChromiumDiagnostic {
            installed: false,
            launchable: false,
            version: None,
        },
        download_policy: "explicit-only".into(),
        checks: [
            "runner",
            "node",
            "node_version",
            "js_dependencies",
            "chromium",
        ]
        .into_iter()
        .map(PrerequisiteCheck::skipped)
        .collect(),
        diagnostic: None,
    };
    let mut diagnostic = DoctorDiagnostic {
        reference: "browser-doctor-latest".into(),
        ..Default::default()
    };
    inspect_prerequisites(&mut report, &mut diagnostic, backend).await;
    report.ready = report.checks.iter().all(|check| check.status == "passed");
    if !report.ready {
        report.diagnostic = Some(diagnostic);
        // A full/unwritable/unsafe cache must never hide prerequisite results.
        let stored = save_diagnostic(&report).is_ok();
        report.diagnostic.as_mut().unwrap().stored = stored;
    }
    report
}

async fn inspect_prerequisites(
    report: &mut BrowserDoctor,
    diagnostic: &mut DoctorDiagnostic,
    backend: BackendId,
) {
    let runner = runner_path();
    report.checks[0].set(runner.as_ref().err().copied());
    let node = match node_binary() {
        Ok(node) => node,
        Err(cause) => {
            report.checks[1].set(Some(cause));
            return;
        }
    };
    let output = match probe(
        &node,
        &[std::ffi::OsStr::new("--version")],
        Duration::from_secs(2),
    )
    .await
    {
        Ok(output) => output,
        Err(cause) => {
            report.checks[1].set(Some(cause));
            return;
        }
    };
    report.checks[1].set(None);
    diagnostic.capture(&output);
    let version = std::str::from_utf8(&output.stdout)
        .ok()
        .map(str::trim)
        .and_then(|version| version.strip_prefix('v'));
    let version =
        match version.filter(|version| version.len() <= 80 && numeric_version(version).is_some()) {
            Some(version) if output.status.success() && !output.stdout_truncated => version,
            _ => {
                report.checks[2].set(Some(Cause::NodeProbeFailed));
                return;
            }
        };
    report.node_version = version.into();
    report.node_supported = numeric_version(version).is_some_and(|version| version >= [22, 22, 2]);
    report.checks[2].set((!report.node_supported).then_some(Cause::NodeUnsupported));
    if !report.node_supported {
        return;
    }
    let Ok(runner) = runner else {
        return;
    };
    let output = match probe(
        &node,
        &[runner.as_os_str(), std::ffi::OsStr::new("doctor")],
        Duration::from_secs(20),
    )
    .await
    {
        Ok(output) => output,
        Err(cause) => {
            report.checks[3].set(Some(cause));
            return;
        }
    };
    diagnostic.capture(&output);
    if !output.status.success() {
        let cause = match diagnostic.error_kind.as_deref() {
            Some("ERR_MODULE_NOT_FOUND" | "MODULE_NOT_FOUND" | "ERR_PACKAGE_PATH_NOT_EXPORTED") => {
                Cause::JsDependenciesMissing
            }
            _ => Cause::RunnerFailed,
        };
        report.checks[3].set(Some(cause));
        return;
    }
    let parsed = (!output.stdout_truncated
        && output.stdout.iter().filter(|byte| **byte == b'\n').count() == 1)
        .then(|| serde_json::from_slice::<RunnerEnvelope>(&output.stdout).ok())
        .flatten()
        .filter(|envelope| envelope.ok)
        .and_then(|envelope| envelope.data)
        .and_then(|data| parse_doctor(data, backend).ok())
        .filter(|runner| runner.node_version == report.node_version);
    let Some(parsed) = parsed else {
        report.checks[3].set(Some(Cause::RunnerOutputInvalid));
        return;
    };
    report.checks[3].set(None);
    report.playwright_version = parsed.playwright_version;
    report.chromium = parsed.chromium;
    report.checks[4].set(
        (!report.chromium.installed || !report.chromium.launchable)
            .then_some(Cause::ChromiumUnavailable),
    );
}

async fn probe(
    node: &Path,
    arguments: &[&std::ffi::OsStr],
    timeout: Duration,
) -> Result<process::CommandOutput, Cause> {
    let mut command = tokio::process::Command::new(node);
    command.args(arguments);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    process::output(
        command,
        CommandPolicy::new(timeout, CAPTURE_LIMIT),
        None,
        "browser prerequisite",
    )
    .await
    .map_err(|error| {
        if error.kind() == CommandFailureKind::Timeout {
            return Cause::ProbeTimeout;
        }
        if error.kind() != CommandFailureKind::Spawn {
            return Cause::NodeProbeFailed;
        }
        match error
            .source()
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind)
        {
            Some(std::io::ErrorKind::NotFound) => Cause::NodeMissing,
            Some(std::io::ErrorKind::PermissionDenied) => Cause::NodeSpawnDenied,
            _ => Cause::NodeSpawnFailed,
        }
    })
}

pub(super) fn is_browser_version(version: &str) -> bool {
    version.len() <= 80
        && version.split('.').count() == 4
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn save_diagnostic(report: &BrowserDoctor) -> Result<(), String> {
    let paths = AppPaths::discover_for_cli()?;
    paths.ensure_cache_directory()?;
    let root = paths.cache_directory().join(STORE_DIRECTORY);
    ensure_private_directory(&root)?;
    let directory = root.join("doctor");
    ensure_private_directory(&directory)?;
    let mut saved = report.clone();
    saved.diagnostic.as_mut().unwrap().stored = true;
    let bytes = serde_json::to_vec(&saved).map_err(|_| "diagnostic encoding failed")?;
    if bytes.len() > 16 * 1024 {
        return Err("diagnostic exceeds limit".into());
    }
    // Atomic replacement bounds retention to one report without following the
    // destination symlink or exposing a partially written diagnostic.
    let mut file =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "diagnostic creation failed")?;
    set_file_permissions(file.path())?;
    file.write_all(&bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "diagnostic write failed")?;
    file.persist(root.join("doctor/latest.json"))
        .map_err(|_| "diagnostic replacement failed")?;
    Ok(())
}
