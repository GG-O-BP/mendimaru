const LAUNCH_STUDIO_TEMPLATE: &str = include_str!("../../scripts/launch_studio.ps1");
const INSTALL_STUDIO_TEMPLATE: &str = include_str!("../../scripts/install_studio.ps1");
const UNINSTALL_STUDIO_TEMPLATE: &str = include_str!("../../scripts/uninstall_studio.ps1");
const STUDIO_SESSIONS_TEMPLATE: &str = include_str!("../../scripts/studio_sessions.ps1");
const OPERATION_SECURITY_PREAMBLE: &str = include_str!("../../scripts/operation_security.ps1");

use crate::models::StudioVersion;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;

pub(super) fn launch_studio_script(
    executable_path: &str,
    project_path: Option<&str>,
    windows_report_path: &str,
    install_root: &str,
    version: &str,
) -> String {
    LAUNCH_STUDIO_TEMPLATE
        .replace("__EXECUTABLE_PATH__", &powershell_literal(executable_path))
        .replace(
            "__PROJECT_PATH__",
            &powershell_literal(project_path.unwrap_or_default()),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
        .replace("__SECURITY_PREAMBLE__", OPERATION_SECURITY_PREAMBLE)
}

pub(super) fn install_script(
    windows_installer_path: &str,
    windows_report_path: &str,
    install_root: &str,
    version: &str,
    expected_sha256: &str,
    source_root: &str,
) -> String {
    INSTALL_STUDIO_TEMPLATE
        .replace(
            "__INSTALLER_PATH__",
            &powershell_literal(windows_installer_path),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
        .replace("__EXPECTED_SHA256__", expected_sha256)
        .replace("__SOURCE_ROOT__", &powershell_literal(source_root))
        .replace("__SECURITY_PREAMBLE__", OPERATION_SECURITY_PREAMBLE)
}

pub(super) fn uninstall_script(
    data_root: &str,
    install_root: &str,
    version: &str,
    windows_report_path: &str,
) -> String {
    UNINSTALL_STUDIO_TEMPLATE
        .replace("__DATA_ROOT__", &powershell_literal(data_root))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__VERSION__", &powershell_literal(version))
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__SECURITY_PREAMBLE__", OPERATION_SECURITY_PREAMBLE)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum StudioSessionScriptMode {
    Query,
    Reconnect { process_id: u32, started_ticks: i64 },
    Stop { process_id: u32, started_ticks: i64 },
}

pub(super) fn studio_sessions_script(
    studios: &[StudioVersion],
    mode: StudioSessionScriptMode,
    windows_report_path: &str,
    install_root: &str,
) -> Result<String, String> {
    #[derive(Serialize)]
    struct KnownStudio<'a> {
        version: &'a str,
        path: &'a str,
    }

    let known = studios
        .iter()
        .map(|studio| KnownStudio {
            version: &studio.version,
            path: &studio.executable_path,
        })
        .collect::<Vec<_>>();
    let known = serde_json::to_vec(&known)
        .map_err(|error| format!("could not serialize known Studio Pro paths: {error}"))?;
    let known = BASE64_STANDARD.encode(known);
    let (mode, process_id, started_ticks) = match mode {
        StudioSessionScriptMode::Query => ("query", 0, 0),
        StudioSessionScriptMode::Reconnect {
            process_id,
            started_ticks,
        } => ("reconnect", process_id, started_ticks),
        StudioSessionScriptMode::Stop {
            process_id,
            started_ticks,
        } => ("stop", process_id, started_ticks),
    };
    Ok(STUDIO_SESSIONS_TEMPLATE
        .replace("__MODE__", mode)
        .replace("__TARGET_PROCESS_ID__", &process_id.to_string())
        .replace("__TARGET_STARTED_TICKS__", &started_ticks.to_string())
        .replace("__KNOWN_STUDIOS_BASE64__", &known)
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
        .replace("__SECURITY_PREAMBLE__", OPERATION_SECURITY_PREAMBLE))
}

pub(super) fn powershell_literal(value: &str) -> String {
    value.replace('\'', "''")
}

pub(super) fn runtime_port_probe_script(guest_port: u16, windows_report_path: &str) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$guestPort = [int]{guest_port}
$resultPath = '{result_path}'

{preamble}

try {{
    $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $guestPort -ErrorAction SilentlyContinue)
    $blockingRules = @(
        Get-NetFirewallRule -Enabled True -Direction Inbound -Action Block -ErrorAction SilentlyContinue |
            Get-NetFirewallPortFilter -ErrorAction SilentlyContinue |
            Where-Object {{
                $_.Protocol -eq 'TCP' -and
                ($_.LocalPort -eq 'Any' -or $_.LocalPort -eq $guestPort.ToString())
            }}
    )
    if ($listeners.Count -eq 0) {{
        $diagnostic = 'MENDIMARU_RUNTIME_NOT_LISTENING'
    }} elseif ($blockingRules.Count -gt 0) {{
        $diagnostic = 'MENDIMARU_RUNTIME_FIREWALL_BLOCKED'
    }} else {{
        $diagnostic = 'MENDIMARU_RUNTIME_LISTENING'
    }}
    $payload = [ordered]@{{
        state = 'succeeded'
        message = $diagnostic
        percentage = $null
        estimated = $false
        timestamp = (Get-Date).ToString('o')
        exitCode = 0
        executablePath = $null
        error = $null
    }}
    Write-MendimaruReport $payload
    exit 0
}} catch {{
    $payload = [ordered]@{{
        state = 'failed'
        message = 'Runtime port diagnosis failed.'
        percentage = $null
        estimated = $false
        timestamp = (Get-Date).ToString('o')
        exitCode = 1
        executablePath = $null
        error = $_.Exception.Message
    }}
    Write-MendimaruReport $payload
    exit 1
}}
"#,
        guest_port = guest_port,
        result_path = powershell_literal(windows_report_path),
        preamble = OPERATION_SECURITY_PREAMBLE,
    )
}

#[cfg(test)]
pub(super) fn security_probe_script(
    target: &str,
    root: &str,
    expected_sha256: &str,
    windows_report_path: &str,
) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$target = '{target}'
$root = '{root}'
$expectedSha256 = '{expected_sha256}'
$resultPath = '{result_path}'

{preamble}

try {{
    $null = Assert-MendimaruTrustedExecutable -Path $target -Root $root -ExpectedSha256 $expectedSha256
    $payload = [ordered]@{{
        state = 'succeeded'
        message = 'Security probe accepted the executable.'
        percentage = $null
        estimated = $false
        timestamp = (Get-Date).ToString('o')
        exitCode = 0
        executablePath = $target
        error = $null
    }}
    Write-MendimaruReport $payload
    exit 0
}} catch {{
    $payload = [ordered]@{{
        state = 'failed'
        message = 'Security probe rejected the executable.'
        percentage = $null
        estimated = $false
        timestamp = (Get-Date).ToString('o')
        exitCode = 1
        executablePath = $target
        error = $_.Exception.Message
    }}
    Write-MendimaruReport $payload
    exit 1
}}
"#,
        target = powershell_literal(target),
        root = powershell_literal(root),
        expected_sha256 = powershell_literal(expected_sha256),
        result_path = powershell_literal(windows_report_path),
        preamble = OPERATION_SECURITY_PREAMBLE,
    )
}

#[cfg(test)]
pub(super) fn reparse_probe_script(windows_report_path: &str) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$resultPath = '{result_path}'

{preamble}

$fixtureRoot = Join-Path $env:ProgramData 'Mendimaru\SecurityFixtures'
$target = Join-Path $fixtureRoot ("target-$mendimaruRequestId")
$junction = Join-Path $fixtureRoot ("junction-$mendimaruRequestId")
$failure = $null
try {{
    $fixtureRoot = New-MendimaruDirectDirectory -Path $fixtureRoot -Root $env:ProgramData
    $target = New-MendimaruDirectDirectory -Path $target -Root $fixtureRoot
    $commandLine = 'mklink /J "' + $junction + '" "' + $target + '"'
    & $env:ComSpec /d /c $commandLine | Out-Null
    if ($LASTEXITCODE -ne 0) {{ throw "MENDIMARU_REPARSE_FIXTURE_CREATE:$LASTEXITCODE" }}
    $rejected = $false
    try {{
        $null = New-MendimaruDirectDirectory -Path (Join-Path $junction 'child') -Root $fixtureRoot
    }} catch {{
        if ($_.Exception.Message.StartsWith('MENDIMARU_REPARSE_POINT:')) {{
            $rejected = $true
        }} else {{
            throw
        }}
    }}
    if (-not $rejected) {{ throw 'MENDIMARU_REPARSE_FIXTURE_ACCEPTED' }}
}} catch {{
    $failure = $_.Exception.Message
}} finally {{
    if (Test-Path -LiteralPath $junction) {{
        & $env:ComSpec /d /c ('rmdir "' + $junction + '"') | Out-Null
    }}
    Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction SilentlyContinue
}}

$payload = [ordered]@{{
    state = if ($null -eq $failure) {{ 'succeeded' }} else {{ 'failed' }}
    message = 'Reparse-point security probe completed.'
    percentage = $null
    estimated = $false
    timestamp = (Get-Date).ToString('o')
    exitCode = if ($null -eq $failure) {{ 0 }} else {{ 1 }}
    executablePath = $null
    error = $failure
}}
Write-MendimaruReport $payload
if ($null -eq $failure) {{ exit 0 }} else {{ exit 1 }}
"#,
        result_path = powershell_literal(windows_report_path),
        preamble = OPERATION_SECURITY_PREAMBLE,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        install_script, launch_studio_script, runtime_port_probe_script, studio_sessions_script,
        uninstall_script, StudioSessionScriptMode,
    };
    use crate::models::StudioVersion;

    #[test]
    fn replaces_every_script_placeholder() {
        let launch = launch_studio_script(
            "studio.exe",
            Some("project.mpr"),
            "launch.json",
            "install-root",
            "11.1.0",
        );
        let install = install_script(
            "setup.exe",
            "install.json",
            "install-root",
            "11.1.0",
            &"ab".repeat(32),
            "share-root",
        );
        let uninstall = uninstall_script("data-root", "install-root", "11.1.0", "remove.json");
        for script in [launch, install, uninstall] {
            assert!(!script.contains("__"), "unreplaced placeholder in script");
        }
    }

    #[test]
    fn generated_scripts_enforce_executable_trust_and_safe_uninstall() {
        let digest = "ab".repeat(32);
        let install = install_script(
            r"\\host.lan\Data\setup.exe",
            r"\\host.lan\Data\install.json",
            r"C:\Program Files\Mendix",
            "11.13.0",
            &digest,
            r"\\host.lan\Data",
        );
        let launch = launch_studio_script(
            r"C:\Program Files\Mendix\11.13.0\modeler\studiopro.exe",
            None,
            r"\\host.lan\Data\launch.json",
            r"C:\Program Files\Mendix",
            "11.13.0",
        );
        let uninstall = uninstall_script(
            r"C:\ProgramData\Mendix",
            r"C:\Program Files\Mendix",
            "11.13.0",
            r"\\host.lan\Data\uninstall.json",
        );

        for script in [&install, &launch, &uninstall] {
            assert!(script.contains("Get-AuthenticodeSignature"));
            assert!(script.contains("-cne 'Valid'"));
            assert!(script.contains("cn=mendix technology b.v."));
            assert!(script.contains("cn=siemens ag"));
            assert!(script.contains("Get-MendimaruSha256"));
            assert!(script.contains("ReparsePoint"));
            assert!(script.contains("Write-MendimaruReport"));
        }
        assert!(install.matches("ExpectedSha256 $expectedSha256").count() >= 3);
        assert!(install.matches("Assert-StudioVersionNotRunning").count() >= 3);
        assert!(install.contains("MENDIMARU_STUDIO_RUNNING"));
        assert!(install.contains("[System.IO.FileMode]::CreateNew"));
        assert!(!install.contains("Unblock-File"));
        assert!(!install.contains("$destinationInfo.Length -eq $sourceInfo.Length"));
        assert!(uninstall.contains("MENDIMARU_UNINSTALL_METADATA_MISSING"));
        assert!(uninstall.contains("MENDIMARU_STUDIO_RUNNING"));
        assert!(!uninstall.contains("Stop-Process"));
        assert!(!uninstall.contains("Remove-Item -LiteralPath $versionFolder -Recurse"));
    }

    #[test]
    fn session_scripts_embed_only_validated_modes_and_exact_process_identity() {
        let studio = StudioVersion {
            version: "11.13.0".into(),
            display_name: "Studio Pro 11.13.0".into(),
            executable_path: r"C:\Program Files\Mendix\11.13.0\modeler\studiopro.exe".into(),
            install_root: r"C:\Program Files\Mendix\11.13.0".into(),
            source: "fixture".into(),
            removable: true,
        };
        let script = studio_sessions_script(
            &[studio],
            StudioSessionScriptMode::Stop {
                process_id: 4242,
                started_ticks: 638_908_128_000_000_000,
            },
            r"\\host.lan\Data\session.json",
            r"C:\Program Files\Mendix",
        )
        .expect("session script");

        assert!(script.contains("$mode = 'stop'"));
        assert!(script.contains("$targetProcessId = [int]4242"));
        assert!(script.contains("$targetStartedTicks = [long]638908128000000000"));
        assert!(script.contains("ProcessSecurity]::IsCurrentUser"));
        assert!(script.contains("return $sessions.ToArray()"));
        assert!(script.contains("Assert-MendimaruTrustedExecutable"));
        assert!(script.contains("CloseMainWindow()"));
        assert!(!script.contains("Stop-Process"));
        assert!(!script.contains("__"));
    }

    #[test]
    fn runtime_probe_uses_a_numeric_port_and_authenticated_closed_report() {
        let script = runtime_port_probe_script(8080, r"\\host.lan\Data\probe.json");
        assert!(script.contains("$guestPort = [int]8080"));
        assert!(script.contains("Get-NetTCPConnection"));
        assert!(script.contains("Get-NetFirewallRule"));
        assert!(script.contains("MENDIMARU_RUNTIME_NOT_LISTENING"));
        assert!(script.contains("MENDIMARU_RUNTIME_FIREWALL_BLOCKED"));
        assert!(script.contains("Write-MendimaruReport"));
        assert!(!script.contains("__"));
    }
}
