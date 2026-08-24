[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('msi', 'nsis')]
    [string]$InstallerKind,

    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [ValidateRange(1, 2147483647)]
    [long]$MaxPrivateMemoryBytes = 1073741824,

    [ValidateRange(0.0, 100.0)]
    [double]$MaxIdleCpuPercent = 10.0,

    [ValidateRange(5, 180)]
    [int]$IdleSettleTimeoutSeconds = 60,

    [switch]$HelpersOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-BundleSmoke {
$expectedGuard = 'mendimaru-github-hosted-ephemeral-vm'
if (
    $env:GITHUB_ACTIONS -ne 'true' -or
    $env:MENDIMARU_EPHEMERAL_WINDOWS_VM -ne $expectedGuard
) {
    throw 'Bundle installation smoke tests are restricted to a marked GitHub-hosted ephemeral Windows VM.'
}
if ([string]::IsNullOrWhiteSpace($env:GITHUB_WORKSPACE)) {
    throw 'GITHUB_WORKSPACE is required.'
}

$workspace = [IO.Path]::GetFullPath($env:GITHUB_WORKSPACE)
$bundleRoot = [IO.Path]::GetFullPath(
    (Join-Path $workspace 'src-tauri\target\release\bundle')
)
$installer = [IO.Path]::GetFullPath($InstallerPath)
if (-not (Test-PathWithin -Path $installer -Root $bundleRoot)) {
    throw "Installer must be under the current workspace bundle directory: $installer"
}
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "Installer does not exist: $installer"
}
$expectedExtension = if ($InstallerKind -eq 'msi') { '.msi' } else { '.exe' }
if ([IO.Path]::GetExtension($installer) -ne $expectedExtension) {
    throw "$InstallerKind smoke test received an unexpected file type: $installer"
}

$artifactDirectory = Join-Path $workspace 'artifacts\bundle-smoke'
New-Item -ItemType Directory -Path $artifactDirectory -Force | Out-Null
$reportPath = Join-Path $artifactDirectory "$InstallerKind.json"
$installLog = Join-Path $artifactDirectory "$InstallerKind-install.log"
$uninstallLog = Join-Path $artifactDirectory "$InstallerKind-uninstall.log"
$report = [ordered]@{
    status = 'running'
    installerKind = $InstallerKind
    installer = $installer
    startedAt = [DateTimeOffset]::UtcNow.ToString('O')
    thresholds = [ordered]@{
        privateMemoryBytes = $MaxPrivateMemoryBytes
        idleCpuPercent = $MaxIdleCpuPercent
        idleSettleTimeoutSeconds = $IdleSettleTimeoutSeconds
    }
    measurements = [ordered]@{}
}
$applicationProcess = $null

try {
    $existingEntries = @(Get-MendimaruUninstallEntries)
    if ($existingEntries.Count -ne 0) {
        throw 'Refusing to overwrite a pre-existing Mendimaru installation.'
    }

    $installStarted = [Diagnostics.Stopwatch]::StartNew()
    if ($InstallerKind -eq 'msi') {
        $installExitCode = Invoke-ExactProcess -FilePath "$env:SystemRoot\System32\msiexec.exe" -Arguments @(
            '/i', $installer, '/qn', '/norestart', '/L*v', $installLog
        )
    }
    else {
        $installExitCode = Invoke-ExactProcess -FilePath $installer -Arguments @('/S')
    }
    $installStarted.Stop()
    if ($installExitCode -notin @(0, 1641, 3010)) {
        throw "Installer returned exit code $installExitCode."
    }
    $report.measurements.installMs = $installStarted.ElapsedMilliseconds
    $report.installExitCode = $installExitCode

    $entry = Wait-MendimaruEntry -Present $true -TimeoutSeconds 45
    $application = Resolve-MendimaruExecutable -Entry $entry
    $report.application = $application
    $file = Get-Item -LiteralPath $application
    if ($file.Name -ine 'mendimaru.exe') {
        throw "Unexpected installed executable: $application"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:MENDIMARU_WINDOWS_CERTIFICATE_THUMBPRINT)) {
        $installedSignature = Get-AuthenticodeSignature -LiteralPath $application
        if ($installedSignature.Status -ne [Management.Automation.SignatureStatus]::Valid) {
            throw "Installed application has an invalid Authenticode signature: $($installedSignature.StatusMessage)"
        }
        if (
            $installedSignature.SignerCertificate.Thumbprint -ne
            $env:MENDIMARU_WINDOWS_CERTIFICATE_THUMBPRINT
        ) {
            throw "Installed application was signed by an unexpected certificate."
        }
        if ($null -eq $installedSignature.TimeStamperCertificate) {
            throw "Installed application signature has no trusted timestamp."
        }
        $report.applicationSignature = 'valid'
    }

    $launchStarted = [Diagnostics.Stopwatch]::StartNew()
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $application
    $startInfo.UseShellExecute = $false
    $applicationProcess = [Diagnostics.Process]::Start($startInfo)
    if ($null -eq $applicationProcess) {
        throw 'Installed application did not start.'
    }
    $windowDeadline = [DateTimeOffset]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 250
        $applicationProcess.Refresh()
        if ($applicationProcess.HasExited) {
            throw "Installed application exited early with code $($applicationProcess.ExitCode)."
        }
    } while (
        $applicationProcess.MainWindowHandle -eq [IntPtr]::Zero -and
        [DateTimeOffset]::UtcNow -lt $windowDeadline
    )
    if ($applicationProcess.MainWindowHandle -eq [IntPtr]::Zero) {
        throw 'Installed application did not create a native window within 30 seconds.'
    }
    $launchStarted.Stop()
    $report.measurements.launchMs = $launchStarted.ElapsedMilliseconds

    $logicalProcessors = [Environment]::ProcessorCount
    $settleStarted = [Diagnostics.Stopwatch]::StartNew()
    $beforeIdle = Get-ProcessTreeSnapshot -RootProcessId $applicationProcess.Id
    $peakProcessCount = $beforeIdle.ProcessCount
    $peakPrivateMemoryBytes = $beforeIdle.PrivateMemoryBytes
    $peakWorkingSetBytes = $beforeIdle.WorkingSetBytes
    $idleCpuPercent = [double]::PositiveInfinity
    do {
        $sampleTimer = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds 5
        $sampleTimer.Stop()
        $applicationProcess.Refresh()
        if ($applicationProcess.HasExited) {
            throw "Installed application exited during the idle performance sample with code $($applicationProcess.ExitCode)."
        }
        $afterIdle = Get-ProcessTreeSnapshot -RootProcessId $applicationProcess.Id
        $peakProcessCount = [Math]::Max($peakProcessCount, $afterIdle.ProcessCount)
        $peakPrivateMemoryBytes = [Math]::Max(
            $peakPrivateMemoryBytes,
            $afterIdle.PrivateMemoryBytes
        )
        $peakWorkingSetBytes = [Math]::Max(
            $peakWorkingSetBytes,
            $afterIdle.WorkingSetBytes
        )
        $idleCpuPercent = [Math]::Round(
            (
                [Math]::Max(0, $afterIdle.CpuSeconds - $beforeIdle.CpuSeconds) /
                ($sampleTimer.Elapsed.TotalSeconds * $logicalProcessors)
            ) * 100,
            2
        )
        $beforeIdle = $afterIdle
    } while (
        $idleCpuPercent -gt $MaxIdleCpuPercent -and
        $settleStarted.Elapsed.TotalSeconds -lt $IdleSettleTimeoutSeconds
    )
    $settleStarted.Stop()
    $report.measurements.idleSettleMs = $settleStarted.ElapsedMilliseconds
    $report.measurements.peakProcessCount = $peakProcessCount
    $report.measurements.peakPrivateMemoryBytes = $peakPrivateMemoryBytes
    $report.measurements.peakWorkingSetBytes = $peakWorkingSetBytes
    $report.measurements.idleCpuPercent = $idleCpuPercent
    if ($peakPrivateMemoryBytes -gt $MaxPrivateMemoryBytes) {
        throw "Installed application process tree exceeded the peak private-memory threshold."
    }
    if ($idleCpuPercent -gt $MaxIdleCpuPercent) {
        throw "Installed application process tree exceeded the idle-CPU threshold."
    }

    Stop-Process -Id $applicationProcess.Id -Force
    $applicationProcess.WaitForExit(10000) | Out-Null
    $applicationProcess = $null

    $uninstallStarted = [Diagnostics.Stopwatch]::StartNew()
    if ($InstallerKind -eq 'msi') {
        $uninstallExitCode = Invoke-ExactProcess -FilePath "$env:SystemRoot\System32\msiexec.exe" -Arguments @(
            '/x', $installer, '/qn', '/norestart', '/L*v', $uninstallLog
        )
    }
    else {
        $installRoot = [IO.Path]::GetDirectoryName($application)
        $uninstallers = @(
            Get-ChildItem -LiteralPath $installRoot -Filter '*.exe' -File |
                Where-Object { $_.BaseName -match '^uninstall' }
        )
        if ($uninstallers.Count -ne 1) {
            throw "Expected exactly one NSIS uninstaller under $installRoot."
        }
        $uninstaller = [IO.Path]::GetFullPath($uninstallers[0].FullName)
        if (-not (Test-PathWithin -Path $uninstaller -Root $installRoot)) {
            throw "Unsafe NSIS uninstaller path: $uninstaller"
        }
        $uninstallExitCode = Invoke-ExactProcess -FilePath $uninstaller -Arguments @('/S')
    }
    $uninstallStarted.Stop()
    if ($uninstallExitCode -notin @(0, 1641, 3010)) {
        throw "Uninstaller returned exit code $uninstallExitCode."
    }
    $report.measurements.uninstallMs = $uninstallStarted.ElapsedMilliseconds
    $report.uninstallExitCode = $uninstallExitCode

    Wait-MendimaruEntry -Present $false -TimeoutSeconds 45 | Out-Null
    $fileDeadline = [DateTimeOffset]::UtcNow.AddSeconds(30)
    while (
        (Test-Path -LiteralPath $application) -and
        [DateTimeOffset]::UtcNow -lt $fileDeadline
    ) {
        Start-Sleep -Milliseconds 250
    }
    if (Test-Path -LiteralPath $application) {
        throw "Application executable remains after uninstall: $application"
    }

    $report.status = 'passed'
}
catch {
    $report.status = 'failed'
    $report.error = $_.Exception.ToString()
    throw
}
finally {
    if ($null -ne $applicationProcess -and -not $applicationProcess.HasExited) {
        Stop-Process -Id $applicationProcess.Id -Force -ErrorAction SilentlyContinue
    }
    $report.finishedAt = [DateTimeOffset]::UtcNow.ToString('O')
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8
}
}

function Invoke-ExactProcess {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )
    if (-not (Test-Path -LiteralPath $FilePath -PathType Leaf)) {
        throw "Executable does not exist: $FilePath"
    }
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = [IO.Path]::GetFullPath($FilePath)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    foreach ($argument in $Arguments) {
        $info.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::Start($info)
    if ($null -eq $process) {
        throw "Failed to start $FilePath"
    }
    $process.WaitForExit()
    return $process.ExitCode
}

function Get-MendimaruUninstallEntries {
    $registryPaths = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    foreach ($registryPath in $registryPaths) {
        Get-ItemProperty -Path $registryPath -ErrorAction SilentlyContinue |
            Where-Object { Test-MendimaruUninstallEntry -Entry $_ }
    }
}

function Test-MendimaruUninstallEntry {
    param([Parameter(Mandatory = $true)]$Entry)
    $displayName = $Entry.PSObject.Properties['DisplayName']
    if ($null -eq $displayName) {
        return $false
    }
    return [string]$displayName.Value -ieq 'mendimaru'
}

function Get-ProcessTreeSnapshot {
    param([Parameter(Mandatory = $true)][int]$RootProcessId)

    $records = @(Get-CimInstance Win32_Process)
    $treeRecords = @(Get-ProcessTreeRecords `
        -Records $records `
        -RootProcessId $RootProcessId)

    $cpuSeconds = 0.0
    $privateMemoryBytes = [long]0
    $workingSetBytes = [long]0
    $sampled = 0
    foreach ($record in $treeRecords) {
        $processId = [int]$record.ProcessId
        try {
            $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
            if ($null -eq $process) {
                continue
            }
            $process.Refresh()
            $cpuSeconds += $process.TotalProcessorTime.TotalSeconds
            $privateMemoryBytes += $process.PrivateMemorySize64
            $workingSetBytes += $process.WorkingSet64
            $sampled += 1
        }
        catch {
            # Short-lived WebView2 children may exit between CIM discovery and sampling.
            continue
        }
    }
    if ($sampled -eq 0) {
        throw "Could not sample application process tree rooted at PID $RootProcessId."
    }

    return [PSCustomObject]@{
        CpuSeconds = $cpuSeconds
        PrivateMemoryBytes = $privateMemoryBytes
        WorkingSetBytes = $workingSetBytes
        ProcessCount = $sampled
    }
}

function Get-ProcessTreeRecords {
    param(
        [Parameter(Mandatory = $true)][object[]]$Records,
        [Parameter(Mandatory = $true)][int]$RootProcessId
    )

    $rootRecords = @(
        $Records | Where-Object { [int]$_.ProcessId -eq $RootProcessId }
    )
    if ($rootRecords.Count -ne 1) {
        throw "Expected one live process record for root PID $RootProcessId; found $($rootRecords.Count)."
    }

    $processIds = [Collections.Generic.HashSet[int]]::new()
    $selected = [Collections.Generic.List[object]]::new()
    $pending = [Collections.Generic.Queue[object]]::new()
    $processIds.Add($RootProcessId) | Out-Null
    $selected.Add($rootRecords[0])
    $pending.Enqueue($rootRecords[0])
    while ($pending.Count -gt 0) {
        $parent = $pending.Dequeue()
        $parentId = [int]$parent.ProcessId
        $parentCreatedAt = [DateTimeOffset]$parent.CreationDate
        foreach ($record in $records) {
            $candidateId = [int]$record.ProcessId
            if (
                [int]$record.ParentProcessId -eq $parentId -and
                [DateTimeOffset]$record.CreationDate -ge $parentCreatedAt -and
                $processIds.Add($candidateId)
            ) {
                $selected.Add($record)
                $pending.Enqueue($record)
            }
        }
    }

    return $selected
}

function Wait-MendimaruEntry {
    param(
        [Parameter(Mandatory = $true)][bool]$Present,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds
    )
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $entries = @(Get-MendimaruUninstallEntries)
        if ($Present -and $entries.Count -eq 1) {
            return $entries[0]
        }
        if (-not $Present -and $entries.Count -eq 0) {
            return $null
        }
        if ($entries.Count -gt 1) {
            throw 'Multiple Mendimaru uninstall entries were detected.'
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTimeOffset]::UtcNow -lt $deadline)
    throw "Timed out waiting for Mendimaru registry presence=$Present."
}

function Resolve-MendimaruExecutable {
    param([Parameter(Mandatory = $true)]$Entry)
    $candidates = [Collections.Generic.List[string]]::new()
    $installLocation = $Entry.PSObject.Properties['InstallLocation']
    $installRoot = if ($null -eq $installLocation) {
        ''
    }
    else {
        ([string]$installLocation.Value).Trim().Trim('"')
    }
    if (-not [string]::IsNullOrWhiteSpace($installRoot)) {
        $candidates.Add((Join-Path $installRoot 'mendimaru.exe'))
    }
    $displayIcon = $Entry.PSObject.Properties['DisplayIcon']
    if (
        $null -ne $displayIcon -and
        -not [string]::IsNullOrWhiteSpace([string]$displayIcon.Value)
    ) {
        $iconPath = ([string]$displayIcon.Value -replace ',\d+$', '').Trim().Trim('"')
        $candidates.Add($iconPath)
    }
    $candidates.Add((Join-Path $env:ProgramFiles 'mendimaru\mendimaru.exe'))
    $candidates.Add((Join-Path $env:LOCALAPPDATA 'mendimaru\mendimaru.exe'))
    $candidates.Add((Join-Path $env:LOCALAPPDATA 'Programs\mendimaru\mendimaru.exe'))

    $existing = @(
        $candidates |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object { [IO.Path]::GetFullPath($_) } |
            Select-Object -Unique |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }
    )
    if ($existing.Count -ne 1) {
        throw "Expected exactly one installed Mendimaru executable; found $($existing.Count)."
    }
    return $existing[0]
}

function Test-PathWithin {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Root
    )
    $normalizedPath = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $normalizedRoot = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    return (
        $normalizedPath -ieq $normalizedRoot -or
        $normalizedPath.StartsWith("$normalizedRoot\", [StringComparison]::OrdinalIgnoreCase)
    )
}

if (-not $HelpersOnly) {
    Invoke-BundleSmoke
}
