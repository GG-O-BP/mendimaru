use super::security;
use crate::contracts::{
    StudioConnectionState, StudioProcessState, StudioReconnectUnavailable, StudioSessionStatus,
    CONTRACT_SCHEMA_VERSION,
};
use crate::models::{AppConfig, StudioVersion};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::{Command, Stdio};

const WINDOWS_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i64 = 10_000_000;
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_PROJECT_NAME_BYTES: usize = 160;

const SESSION_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$mode = $env:MENDIMARU_SESSION_MODE
$targetProcessId = [int]$env:MENDIMARU_SESSION_PROCESS_ID
$targetStartedTicks = [long]$env:MENDIMARU_SESSION_STARTED_TICKS
$knownJson = [Text.Encoding]::UTF8.GetString(
  [Convert]::FromBase64String($env:MENDIMARU_KNOWN_STUDIOS)
)
$knownStudios = @($knownJson | ConvertFrom-Json)

if (-not ('Mendimaru.ProcessSecurity' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Security.Principal;

namespace Mendimaru {
  public static class ProcessSecurity {
    private const uint QueryLimitedInformation = 0x1000;
    private const uint TokenQuery = 0x0008;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr handle);

    public static bool IsCurrentUser(int processId) {
      IntPtr process = OpenProcess(QueryLimitedInformation, false, (uint)processId);
      if (process == IntPtr.Zero) return false;
      IntPtr token = IntPtr.Zero;
      try {
        if (!OpenProcessToken(process, TokenQuery, out token) || token == IntPtr.Zero) return false;
        using (WindowsIdentity owner = new WindowsIdentity(token)) {
          SecurityIdentifier current = WindowsIdentity.GetCurrent().User;
          return current != null && current.Equals(owner.User);
        }
      } catch {
        return false;
      } finally {
        if (token != IntPtr.Zero) CloseHandle(token);
        CloseHandle(process);
      }
    }
  }
}
'@
}

function Get-ProjectName {
  param($NativeProcess)
  $name = $null
  try {
    $commandLine = (Get-CimInstance Win32_Process -Filter "ProcessId = $($NativeProcess.Id)" -ErrorAction Stop).CommandLine
    if (-not [string]::IsNullOrWhiteSpace($commandLine)) {
      $match = [regex]::Match(
        $commandLine,
        '(?i)(?:"([^"\r\n]+\.mpr)"|([^\s"\r\n]+\.mpr))'
      )
      if ($match.Success) {
        $path = if ($match.Groups[1].Success) { $match.Groups[1].Value } else { $match.Groups[2].Value }
        $name = [IO.Path]::GetFileNameWithoutExtension($path)
      }
    }
  } catch { $name = $null }
  if ([string]::IsNullOrWhiteSpace($name)) {
    $titleMatch = [regex]::Match(
      [string]$NativeProcess.MainWindowTitle,
      '^(.*?)\s+-\s+Mendix Studio Pro(?:\s|$)'
    )
    if ($titleMatch.Success) { $name = $titleMatch.Groups[1].Value }
  }
  if ([string]::IsNullOrWhiteSpace($name) -or $name.Length -gt 160 -or
      $name.IndexOfAny([char[]]@(0, 10, 13)) -ge 0) { return $null }
  return $name
}

function Get-StudioSessions {
  $sessions = New-Object 'System.Collections.Generic.List[object]'
  $processes = @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue)
  foreach ($candidate in $processes) {
    if ($sessions.Count -ge 64 -or
        -not [Mendimaru.ProcessSecurity]::IsCurrentUser([int]$candidate.Id)) { continue }
    try {
      $native = Get-Process -Id ([int]$candidate.Id) -ErrorAction Stop
      $native.Refresh()
      $executablePath = [string]$native.Path
      if ([string]::IsNullOrWhiteSpace($executablePath)) { continue }
      $known = @($knownStudios | Where-Object {
        $_.path.Equals($executablePath, [StringComparison]::OrdinalIgnoreCase)
      } | Select-Object -First 1)
      if ($known.Count -ne 1) { continue }
      $started = $native.StartTime.ToUniversalTime()
      $ticks = $started.Ticks
      if ($targetProcessId -gt 0 -and
          ($native.Id -ne $targetProcessId -or $ticks -ne $targetStartedTicks)) { continue }
      $sessions.Add([ordered]@{
        sessionId = 'studio-{0}-{1}' -f @($native.Id, $ticks)
        version = [string]$known[0].version
        processId = [int]$native.Id
        startedAt = $started.ToString('o')
        projectName = Get-ProjectName $native
        hasWindow = $native.MainWindowHandle -ne [IntPtr]::Zero
      })
    } catch { continue }
  }
  return $sessions.ToArray()
}

function Get-Target {
  $matches = @(Get-StudioSessions)
  if ($matches.Count -ne 1) { throw 'MENDIMARU_STUDIO_SESSION_ENDED' }
  return $matches[0]
}

switch ($mode) {
  'query' {
    $json = ConvertTo-Json -InputObject @(Get-StudioSessions) -Compress -Depth 5
    [Console]::OutputEncoding = [Text.Encoding]::UTF8
    [Console]::Out.Write($json)
    exit 0
  }
  'reconnect' {
    $session = Get-Target
    if (-not $session.hasWindow) { throw 'MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE' }
    if (-not ('Mendimaru.WindowControl' -as [type])) {
      Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace Mendimaru {
  public static class WindowControl {
    [DllImport("user32.dll")]
    public static extern bool ShowWindowAsync(IntPtr window, int command);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);
  }
}
'@
    }
    $process = Get-Process -Id ([int]$session.processId) -ErrorAction Stop
    $process.Refresh()
    $null = [Mendimaru.WindowControl]::ShowWindowAsync($process.MainWindowHandle, 9)
    $null = [Mendimaru.WindowControl]::SetForegroundWindow($process.MainWindowHandle)
    exit 0
  }
  'stop' {
    $session = Get-Target
    if (-not $session.hasWindow) { throw 'MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE' }
    $process = Get-Process -Id ([int]$session.processId) -ErrorAction Stop
    $process.Refresh()
    if (-not $process.CloseMainWindow()) { throw 'MENDIMARU_STUDIO_SESSION_CLOSE_REJECTED' }
    $deadline = (Get-Date).AddSeconds(30)
    do {
      Start-Sleep -Milliseconds 500
      $process.Refresh()
    } while (-not $process.HasExited -and (Get-Date) -lt $deadline)
    if (-not $process.HasExited) { throw 'MENDIMARU_STUDIO_SESSION_CLOSE_PENDING' }
    exit 0
  }
  default { throw 'MENDIMARU_STUDIO_SESSION_MODE_INVALID' }
}
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KnownStudio<'a> {
    version: &'a str,
    path: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeSessionReport {
    session_id: String,
    version: String,
    process_id: u32,
    started_at: String,
    #[serde(default)]
    project_name: Option<String>,
    has_window: bool,
}

#[derive(Debug, Clone, Copy)]
enum SessionMode {
    Query,
    Reconnect(SessionIdentity),
    Stop(SessionIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionIdentity {
    process_id: u32,
    started_ticks: i64,
}

pub(super) fn list(config: &AppConfig) -> Result<Vec<StudioSessionStatus>, String> {
    let studios = super::installed_versions(config)?;
    let output = run_script(&studios, SessionMode::Query)?;
    normalize_output(&studios, &output)
}

pub(super) fn reconnect(config: &AppConfig, session_id: &str) -> Result<(), String> {
    let identity = parse_session_id(session_id)?;
    let session = list(config)?
        .into_iter()
        .find(|session| session.session_id == session_id)
        .ok_or_else(|| crate::tr!("error-script-studio-session-ended"))?;
    if !session.reconnectable {
        return Err(crate::tr!("error-script-studio-session-window-unavailable"));
    }
    let studios = super::installed_versions(config)?;
    run_script(&studios, SessionMode::Reconnect(identity)).map(|_| ())
}

pub(super) fn stop(config: &AppConfig, session_id: &str) -> Result<(), String> {
    let identity = parse_session_id(session_id)?;
    if !list(config)?
        .iter()
        .any(|session| session.session_id == session_id)
    {
        return Err(crate::tr!("error-script-studio-session-ended"));
    }
    let studios = super::installed_versions(config)?;
    run_script(&studios, SessionMode::Stop(identity)).map(|_| ())
}

fn run_script(studios: &[StudioVersion], mode: SessionMode) -> Result<Vec<u8>, String> {
    let known = studios
        .iter()
        .map(|studio| KnownStudio {
            version: &studio.version,
            path: &studio.executable_path,
        })
        .collect::<Vec<_>>();
    let known = serde_json::to_vec(&known)
        .map_err(|error| format!("could not serialize known Studio Pro paths: {error}"))?;
    let (mode, identity) = match mode {
        SessionMode::Query => ("query", None),
        SessionMode::Reconnect(identity) => ("reconnect", Some(identity)),
        SessionMode::Stop(identity) => ("stop", Some(identity)),
    };
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "RemoteSigned",
            "-Command",
            SESSION_SCRIPT,
        ])
        .env("MENDIMARU_SESSION_MODE", mode)
        .env(
            "MENDIMARU_SESSION_PROCESS_ID",
            identity
                .map_or(0, |identity| identity.process_id)
                .to_string(),
        )
        .env(
            "MENDIMARU_SESSION_STARTED_TICKS",
            identity
                .map_or(0, |identity| identity.started_ticks)
                .to_string(),
        )
        .env("MENDIMARU_KNOWN_STUDIOS", BASE64_STANDARD.encode(known))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not inspect Studio Pro sessions: {error}"))?;
    if !output.status.success() {
        return Err(localize_script_error(&output.stderr));
    }
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        return Err("Windows returned too much Studio Pro session data".to_string());
    }
    Ok(output.stdout)
}

fn localize_script_error(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    for (code, message) in [
        (
            "MENDIMARU_STUDIO_SESSION_ENDED",
            crate::tr!("error-script-studio-session-ended"),
        ),
        (
            "MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE",
            crate::tr!("error-script-studio-session-window-unavailable"),
        ),
        (
            "MENDIMARU_STUDIO_SESSION_CLOSE_PENDING",
            crate::tr!("error-script-studio-session-close-pending"),
        ),
        (
            "MENDIMARU_STUDIO_SESSION_CLOSE_REJECTED",
            crate::tr!("error-script-studio-session-close-rejected"),
        ),
    ] {
        if stderr.contains(code) {
            return message;
        }
    }
    crate::tr!("error-script-studio-session-query-failed")
}

fn normalize_output(
    studios: &[StudioVersion],
    output: &[u8],
) -> Result<Vec<StudioSessionStatus>, String> {
    normalize_output_with(studios, output, security::verify_mendix_executable)
}

fn normalize_output_with<V>(
    studios: &[StudioVersion],
    output: &[u8],
    verify: V,
) -> Result<Vec<StudioSessionStatus>, String>
where
    V: Fn(&std::path::Path) -> Result<String, String>,
{
    let reports: Vec<NativeSessionReport> =
        serde_json::from_slice(output.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(output))
            .map_err(|error| {
                format!("Windows returned invalid Studio Pro session data: {error}")
            })?;
    if reports.len() > 64 {
        return Err("Windows returned too many Studio Pro sessions".to_string());
    }
    let mut ids = HashSet::new();
    let mut sessions = Vec::with_capacity(reports.len());
    for report in reports {
        let identity = parse_session_id(&report.session_id)?;
        let studio = studios
            .iter()
            .find(|studio| studio.version == report.version)
            .ok_or_else(|| "Windows returned an unknown Studio Pro version".to_string())?;
        if identity.process_id != report.process_id || !ids.insert(report.session_id.clone()) {
            return Err("Windows returned an invalid Studio Pro session".to_string());
        }
        let started_at = DateTime::parse_from_rfc3339(&report.started_at)
            .map_err(|_| "Windows returned an invalid Studio Pro start time".to_string())?
            .with_timezone(&Utc);
        if datetime_ticks(started_at) != Some(identity.started_ticks) {
            return Err(
                "the Studio Pro process identity does not match its start time".to_string(),
            );
        }
        verify(std::path::Path::new(&studio.executable_path))?;
        let project_name = report
            .project_name
            .filter(|name| safe_project_name(name))
            .map(|name| name.trim().to_string());
        sessions.push(StudioSessionStatus {
            schema_version: CONTRACT_SCHEMA_VERSION.to_string(),
            session_id: report.session_id,
            version: report.version,
            state: StudioProcessState::Running,
            process_id: Some(report.process_id),
            started_at: Some(started_at),
            project_name,
            connection: StudioConnectionState::Native,
            reconnectable: report.has_window,
            reconnect_unavailable: (!report.has_window)
                .then_some(StudioReconnectUnavailable::WindowUnavailable),
        });
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.started_at));
    Ok(sessions)
}

fn parse_session_id(value: &str) -> Result<SessionIdentity, String> {
    let remainder = value
        .strip_prefix("studio-")
        .ok_or_else(|| "the Studio Pro session identifier is invalid".to_string())?;
    let (process_id, started_ticks) = remainder
        .split_once('-')
        .ok_or_else(|| "the Studio Pro session identifier is invalid".to_string())?;
    let process_id = process_id
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "the Studio Pro session identifier is invalid".to_string())?;
    let started_ticks = started_ticks
        .parse::<i64>()
        .ok()
        .filter(|value| *value > WINDOWS_EPOCH_TICKS)
        .ok_or_else(|| "the Studio Pro session identifier is invalid".to_string())?;
    Ok(SessionIdentity {
        process_id,
        started_ticks,
    })
}

fn datetime_ticks(value: DateTime<Utc>) -> Option<i64> {
    WINDOWS_EPOCH_TICKS
        .checked_add(value.timestamp().checked_mul(TICKS_PER_SECOND)?)?
        .checked_add(i64::from(value.timestamp_subsec_nanos() / 100))
}

fn safe_project_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= MAX_PROJECT_NAME_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
}

#[cfg(test)]
mod tests {
    use super::{
        datetime_ticks, normalize_output, normalize_output_with, parse_session_id, StudioVersion,
        SESSION_SCRIPT,
    };
    use chrono::{TimeZone, Utc};

    fn studio() -> StudioVersion {
        StudioVersion {
            version: "11.13.0".into(),
            display_name: "Studio Pro 11.13.0".into(),
            executable_path: std::env::current_exe()
                .expect("test executable")
                .to_string_lossy()
                .to_string(),
            install_root: String::new(),
            source: "fixture".into(),
            removable: true,
        }
    }

    #[test]
    fn session_ids_bind_pid_to_exact_start_time() {
        let started = Utc
            .with_ymd_and_hms(2026, 8, 15, 3, 0, 0)
            .single()
            .expect("time");
        let ticks = datetime_ticks(started).expect("ticks");
        let id = format!("studio-4242-{ticks}");
        assert_eq!(parse_session_id(&id).expect("identity").process_id, 4242);
        assert!(parse_session_id(&format!("studio-0-{ticks}")).is_err());
    }

    #[test]
    fn native_actions_require_current_user_exact_identity_and_never_force_close() {
        assert!(SESSION_SCRIPT.contains("ProcessSecurity]::IsCurrentUser"));
        assert!(SESSION_SCRIPT.contains("$targetStartedTicks"));
        assert!(SESSION_SCRIPT.contains("ShowWindowAsync"));
        assert!(SESSION_SCRIPT.contains("CloseMainWindow()"));
        assert!(SESSION_SCRIPT.contains("return $sessions.ToArray()"));
        assert!(!SESSION_SCRIPT.contains("Stop-Process"));
    }

    #[test]
    fn rejects_unknown_or_mismatched_native_session_results_before_actions() {
        let started = Utc
            .with_ymd_and_hms(2026, 8, 15, 3, 0, 0)
            .single()
            .expect("time");
        let ticks = datetime_ticks(started).expect("ticks");
        let fixture = serde_json::json!([{
            "sessionId": format!("studio-4242-{ticks}"),
            "version": "99.0.0",
            "processId": 4242,
            "startedAt": started.to_rfc3339(),
            "projectName": "Orders",
            "hasWindow": true
        }]);
        assert!(normalize_output(&[studio()], fixture.to_string().as_bytes()).is_err());
    }

    #[test]
    fn normalizes_multiple_native_versions_and_window_capabilities() {
        let first = Utc
            .with_ymd_and_hms(2026, 8, 15, 3, 0, 0)
            .single()
            .expect("first time");
        let second = Utc
            .with_ymd_and_hms(2026, 8, 15, 2, 0, 0)
            .single()
            .expect("second time");
        let first_ticks = datetime_ticks(first).expect("first ticks");
        let second_ticks = datetime_ticks(second).expect("second ticks");
        let mut other = studio();
        other.version = "10.24.9".into();
        let fixture = serde_json::json!([
            {
                "sessionId": format!("studio-5151-{second_ticks}"),
                "version": "10.24.9",
                "processId": 5151,
                "startedAt": second.to_rfc3339(),
                "projectName": null,
                "hasWindow": false
            },
            {
                "sessionId": format!("studio-4242-{first_ticks}"),
                "version": "11.13.0",
                "processId": 4242,
                "startedAt": first.to_rfc3339(),
                "projectName": "Orders",
                "hasWindow": true
            }
        ]);
        let sessions =
            normalize_output_with(&[studio(), other], fixture.to_string().as_bytes(), |_| {
                Ok("verified".into())
            })
            .expect("native sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].version, "11.13.0");
        assert!(sessions[0].reconnectable);
        assert!(!sessions[1].reconnectable);
    }
}
