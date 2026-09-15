param(
    [Parameter(Mandatory = $true)][ValidateRange(1, 2147483647)][int]$StudioProcessId,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9]{15,20}$')][string]$StartTimeUtcTicks,
    [Parameter(Mandatory = $true)][ValidatePattern('^(10|11)\.[0-9]+\.[0-9]+\.[0-9]+$')][string]$FileVersion,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [ValidateRange(5, 60)][int]$TimeoutSeconds = 45
)

# Offline PoC tool, intentionally separate from the product's UI capabilities.
# Run in the same interactive Windows session as the disposable Studio project.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'windows-guest-required' }
# SMB permissions do not necessarily implement Windows ACLs. Collect on local
# NTFS first; copy reviewed artifacts to the host only after the worker exits.
if ($OutputDirectory -notmatch '^[A-Za-z]:\\' -or $OutputDirectory.Substring(2).Contains(':')) { throw 'local-output-required' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$drive = New-Object IO.DriveInfo ([IO.Path]::GetPathRoot($OutputDirectory))
if ($drive.DriveType -ne [IO.DriveType]::Fixed -or $drive.DriveFormat -ne 'NTFS') { throw 'local-ntfs-output-required' }
$ancestor = [IO.Directory]::GetParent($OutputDirectory)
while ($null -ne $ancestor) {
    if (-not $ancestor.Exists) { throw 'output-parent-missing' }
    if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'linked-output-ancestor' }
    $ancestor = $ancestor.Parent
}
if (Test-Path -LiteralPath $OutputDirectory) { throw 'output-already-exists' }
$directory = New-Item -ItemType Directory -Path $OutputDirectory
if ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'linked-output' }
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetAccessRuleProtection($true, $false)
$owner = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl.SetOwner($owner)
foreach ($sid in @($owner, (New-Object Security.Principal.SecurityIdentifier 'S-1-5-18'))) {
    $rule = New-Object Security.AccessControl.FileSystemAccessRule($sid, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
}
Set-Acl -LiteralPath $directory.FullName -AclObject $acl

function Literal([string]$value) { return "'" + $value.Replace("'", "''") + "'" }
$worker = Join-Path $PSScriptRoot 'capture-worker.ps1'
$script = '& ' + (Literal $worker) + ' -StudioProcessId ' + $StudioProcessId +
    ' -StartTimeUtcTicks ' + (Literal $StartTimeUtcTicks) +
    ' -FileVersion ' + (Literal $FileVersion) +
    ' -OutputDirectory ' + (Literal $directory.FullName)
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
$powershell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
function Stop-Worker([Diagnostics.Process]$process) {
    if (-not $process.HasExited) {
        try { $process.Kill() }
        catch {
            # The worker may finish between HasExited and Kill.
            if (-not $process.HasExited) { throw }
        }
    }
    $null = $process.WaitForExit(5000)
}

$child = $null
$result = @{ schemaVersion = 1; status = 'fail'; reason = 'worker-start-failed' }
try {
    # UIA providers can block inside a COM call. A loop deadline alone cannot
    # bound that call, so a separate MTA process owns every UIA/GDI operation.
    $child = Start-Process -FilePath $powershell -ArgumentList @('-NoLogo', '-NoProfile', '-NonInteractive', '-Mta', '-EncodedCommand', $encoded) -WindowStyle Hidden -PassThru
    if (-not $child.WaitForExit($TimeoutSeconds * 1000)) {
        $result.reason = 'provider-timeout'
        Stop-Worker $child
    } elseif ($child.ExitCode -eq 0 -and (Test-Path -LiteralPath (Join-Path $directory.FullName 'snapshot.json'))) {
        $snapshot = Get-Content -LiteralPath (Join-Path $directory.FullName 'snapshot.json') -Raw | ConvertFrom-Json
        if ($snapshot.windowsTruncated -or @($snapshot.windows | Where-Object { $_.truncated }).Count -gt 0) {
            $result.reason = 'partial-capture'
        } else {
            $result.status = 'captured'
            $result.reason = 'requires-artifact-review'
        }
    } else {
        $result.reason = 'worker-failed'
        $failurePath = Join-Path $directory.FullName 'worker-failure.json'
        if (Test-Path -LiteralPath $failurePath) {
            $failure = [IO.File]::ReadAllText($failurePath) | ConvertFrom-Json
            $allowed = @('initialization-failed', 'stale-or-wrong-process', 'no-interactive-desktop',
                'no-visible-window', 'provider-failed', 'desktop-changed-during-capture')
            if ($failure.reason -in $allowed) { $result.reason = [string]$failure.reason }
        }
    }
} finally {
    if ($null -ne $child) {
        try { Stop-Worker $child }
        catch { $result.status = 'fail'; $result.reason = 'worker-cleanup-failed' }
        $result.workerProcessId = $child.Id
        $result.workerExited = $child.HasExited
        if (-not $result.workerExited) { $result.status = 'fail'; $result.reason = 'worker-cleanup-failed' }
        $child.Dispose()
    }
    $result | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $directory.FullName 'supervisor.json') -Encoding UTF8
}
if ($result.status -ne 'captured') { throw $result.reason }
$result | ConvertTo-Json -Compress
