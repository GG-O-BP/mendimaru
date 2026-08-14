const LAUNCH_STUDIO_TEMPLATE: &str = include_str!("../../scripts/launch_studio.ps1");
const INSTALL_STUDIO_TEMPLATE: &str = include_str!("../../scripts/install_studio.ps1");
const UNINSTALL_STUDIO_TEMPLATE: &str = include_str!("../../scripts/uninstall_studio.ps1");
const OPERATION_SECURITY_PREAMBLE: &str = include_str!("../../scripts/operation_security.ps1");

pub(super) fn launch_studio_script(
    executable_path: &str,
    project_path: Option<&str>,
    windows_report_path: &str,
    install_root: &str,
) -> String {
    LAUNCH_STUDIO_TEMPLATE
        .replace("__EXECUTABLE_PATH__", &powershell_literal(executable_path))
        .replace(
            "__PROJECT_PATH__",
            &powershell_literal(project_path.unwrap_or_default()),
        )
        .replace("__RESULT_PATH__", &powershell_literal(windows_report_path))
        .replace("__INSTALL_ROOT__", &powershell_literal(install_root))
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

pub(super) fn powershell_literal(value: &str) -> String {
    value.replace('\'', "''")
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
    use super::{install_script, launch_studio_script, uninstall_script};

    #[test]
    fn replaces_every_script_placeholder() {
        let launch = launch_studio_script(
            "studio.exe",
            Some("project.mpr"),
            "launch.json",
            "install-root",
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
        assert!(install.contains("[System.IO.FileMode]::CreateNew"));
        assert!(!install.contains("Unblock-File"));
        assert!(!install.contains("$destinationInfo.Length -eq $sourceInfo.Length"));
        assert!(uninstall.contains("MENDIMARU_UNINSTALL_METADATA_MISSING"));
        assert!(!uninstall.contains("Remove-Item -LiteralPath $versionFolder -Recurse"));
    }
}
