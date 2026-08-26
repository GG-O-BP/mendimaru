[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('msi', 'nsis')]
    [string]$InstallerKind,

    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [switch]$Performance,

    [switch]$HelpersOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-BundleSmoke {
$performanceSampleCount = if ($Performance) { 7 } else { 1 }
$idleWindowSeconds = if ($Performance) { 300 } else { 5 }
$idleSampleSeconds = 5
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
    schemaVersion = 'raw-1.0.0'
    status = 'running'
    installerKind = $InstallerKind
    installer = $installer
    startedAt = [DateTimeOffset]::UtcNow.ToString('O')
    sampling = [ordered]@{
        warmupCount = 1
        sampleCount = $performanceSampleCount
        idleWindowSeconds = $idleWindowSeconds
        idleSampleSeconds = $idleSampleSeconds
    }
    measurements = [ordered]@{
        coldStartupMs = @()
        warmStartupMs = @()
        idleCpuPercent = @()
        privateMemoryBytes = @()
        workingSetBytes = @()
        processCount = @()
    }
}
$applicationProcess = $null
$webViewData = $null
$applicationData = $null

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

    $webViewData = Join-Path $artifactDirectory "$InstallerKind-webview-data"
    $applicationData = Join-Path $artifactDirectory "$InstallerKind-application-data"
    Remove-Item -LiteralPath $applicationData -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $applicationData -Force | Out-Null
    $warmup = Start-BundleApplication `
        -Application $application `
        -ApplicationData $applicationData `
        -WebViewData $webViewData `
        -ClearWebViewData
    Stop-BundleApplication -Process $warmup.Process

    for ($sample = 0; $sample -lt $performanceSampleCount; $sample += 1) {
        $cold = Start-BundleApplication `
            -Application $application `
            -ApplicationData $applicationData `
            -WebViewData $webViewData `
            -ClearWebViewData
        $report.measurements.coldStartupMs += [long]$cold.ElapsedMilliseconds
        Stop-BundleApplication -Process $cold.Process

        $warm = Start-BundleApplication `
            -Application $application `
            -ApplicationData $applicationData `
            -WebViewData $webViewData
        $report.measurements.warmStartupMs += [long]$warm.ElapsedMilliseconds
        Stop-BundleApplication -Process $warm.Process
    }

    $idleLaunch = Start-BundleApplication `
        -Application $application `
        -ApplicationData $applicationData `
        -WebViewData $webViewData
    $applicationProcess = $idleLaunch.Process
    $report.webviewVersion = Get-WebViewVersion `
        -RootProcessId $applicationProcess.Id
    $logicalProcessors = [Environment]::ProcessorCount
    Start-Sleep -Seconds $idleSampleSeconds
    $cpuTracker = @{
        initialized = $false
        cumulativeSeconds = 0.0
        previous = @{}
    }
    $beforeIdle = Get-ProcessTreeSnapshot `
        -RootProcessId $applicationProcess.Id `
        -CpuTracker $cpuTracker
    $previousIdle = $beforeIdle
    $peakProcessCount = $beforeIdle.ProcessCount
    $peakPrivateMemoryBytes = $beforeIdle.PrivateMemoryBytes
    $peakWorkingSetBytes = $beforeIdle.WorkingSetBytes
    $peakCpuSeconds = $beforeIdle.CpuSeconds
    $idleSamples = [Math]::Ceiling($idleWindowSeconds / $idleSampleSeconds)
    for ($sample = 0; $sample -lt $idleSamples; $sample += 1) {
        $sampleTimer = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds $idleSampleSeconds
        $sampleTimer.Stop()
        $applicationProcess.Refresh()
        if ($applicationProcess.HasExited) {
            throw "Installed application exited during the idle performance sample with code $($applicationProcess.ExitCode)."
        }
        $afterIdle = Get-ProcessTreeSnapshot `
            -RootProcessId $applicationProcess.Id `
            -CpuTracker $cpuTracker
        if ($afterIdle.CpuSeconds -lt $previousIdle.CpuSeconds) {
            throw 'Installed application process-tree CPU time moved backwards.'
        }
        $idleCpuPercent = [Math]::Round(
            (
                ($afterIdle.CpuSeconds - $previousIdle.CpuSeconds) /
                ($sampleTimer.Elapsed.TotalSeconds * $logicalProcessors)
            ) * 100,
            3
        )
        $report.measurements.idleCpuPercent += $idleCpuPercent
        $report.measurements.privateMemoryBytes += [long]$afterIdle.PrivateMemoryBytes
        $report.measurements.workingSetBytes += [long]$afterIdle.WorkingSetBytes
        $report.measurements.processCount += [int]$afterIdle.ProcessCount
        $peakProcessCount = [Math]::Max($peakProcessCount, $afterIdle.ProcessCount)
        $peakPrivateMemoryBytes = [Math]::Max(
            $peakPrivateMemoryBytes,
            $afterIdle.PrivateMemoryBytes
        )
        $peakWorkingSetBytes = [Math]::Max(
            $peakWorkingSetBytes,
            $afterIdle.WorkingSetBytes
        )
        $peakCpuSeconds = [Math]::Max($peakCpuSeconds, $afterIdle.CpuSeconds)
        $previousIdle = $afterIdle
    }
    $report.resources = [ordered]@{
        before = $beforeIdle
        after = $previousIdle
        peak = [ordered]@{
            processCount = $peakProcessCount
            privateMemoryBytes = $peakPrivateMemoryBytes
            workingSetBytes = $peakWorkingSetBytes
            cpuSeconds = $peakCpuSeconds
        }
        delta = [ordered]@{
            processCount = $previousIdle.ProcessCount - $beforeIdle.ProcessCount
            privateMemoryBytes = $previousIdle.PrivateMemoryBytes - $beforeIdle.PrivateMemoryBytes
            workingSetBytes = $previousIdle.WorkingSetBytes - $beforeIdle.WorkingSetBytes
        }
    }

    Stop-BundleApplication -Process $applicationProcess
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
        try {
            Stop-BundleApplication -Process $applicationProcess
        }
        catch {
            Stop-Process -Id $applicationProcess.Id -Force -ErrorAction SilentlyContinue
        }
    }
    if (
        $null -ne $webViewData -and
        (Test-Path -LiteralPath $webViewData)
    ) {
        Remove-Item -LiteralPath $webViewData -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (
        $null -ne $applicationData -and
        (Test-Path -LiteralPath $applicationData)
    ) {
        Remove-Item -LiteralPath $applicationData -Recurse -Force -ErrorAction SilentlyContinue
    }
    $report.finishedAt = [DateTimeOffset]::UtcNow.ToString('O')
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding utf8
}
}

function Start-BundleApplication {
    param(
        [Parameter(Mandatory = $true)][string]$Application,
        [Parameter(Mandatory = $true)][string]$ApplicationData,
        [Parameter(Mandatory = $true)][string]$WebViewData,
        [switch]$ClearWebViewData
    )
    $canonicalData = [IO.Path]::GetFullPath($WebViewData)
    $canonicalApplicationData = [IO.Path]::GetFullPath($ApplicationData)
    $artifactRoot = [IO.Path]::GetFullPath(
        (Join-Path $env:GITHUB_WORKSPACE 'artifacts\bundle-smoke')
    )
    foreach ($directory in @($canonicalData, $canonicalApplicationData)) {
        if (-not (Test-PathWithin -Path $directory -Root $artifactRoot)) {
            throw "Application data must stay under bundle-smoke artifacts: $directory"
        }
    }
    if ($ClearWebViewData) {
        Remove-Item -LiteralPath $canonicalData -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $canonicalData) {
            throw "Cold-start WebView data could not be cleared: $canonicalData"
        }
    }
    New-Item -ItemType Directory -Path $canonicalData -Force | Out-Null
    $roamingData = Join-Path $canonicalApplicationData 'Roaming'
    $localData = Join-Path $canonicalApplicationData 'Local'
    New-Item -ItemType Directory -Path $roamingData, $localData -Force | Out-Null

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = [IO.Path]::GetFullPath($Application)
    $startInfo.UseShellExecute = $false
    $startInfo.Environment['APPDATA'] = $roamingData
    $startInfo.Environment['LOCALAPPDATA'] = $localData
    $startInfo.Environment['WEBVIEW2_USER_DATA_FOLDER'] = $canonicalData
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) {
        throw 'Installed application did not start.'
    }
    $windowDeadline = [DateTimeOffset]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 100
        $process.Refresh()
        if ($process.HasExited) {
            throw "Installed application exited early with code $($process.ExitCode)."
        }
    } while (
        $process.MainWindowHandle -eq [IntPtr]::Zero -and
        [DateTimeOffset]::UtcNow -lt $windowDeadline
    )
    $timer.Stop()
    if ($process.MainWindowHandle -eq [IntPtr]::Zero) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw 'Installed application did not create a native window within 30 seconds.'
    }
    return [PSCustomObject]@{
        Process = $process
        ElapsedMilliseconds = $timer.Elapsed.TotalMilliseconds
    }
}

function Stop-BundleApplication {
    param([Parameter(Mandatory = $true)][Diagnostics.Process]$Process)
    if (-not $Process.HasExited) {
        $taskkill = Join-Path $env:SystemRoot 'System32\taskkill.exe'
        $exitCode = Invoke-ExactProcess `
            -FilePath $taskkill `
            -Arguments @('/PID', [string]$Process.Id, '/T', '/F')
        $Process.Refresh()
        if ($exitCode -ne 0 -and -not $Process.HasExited) {
            throw "Failed to stop installed application tree rooted at PID $($Process.Id)."
        }
    }
    $Process.WaitForExit(10000) | Out-Null
}

function Get-WebViewVersion {
    param([Parameter(Mandatory = $true)][int]$RootProcessId)
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(15)
    do {
        $records = @(Get-CimInstance Win32_Process)
        $tree = @(Get-ProcessTreeRecords `
            -Records $records `
            -RootProcessId $RootProcessId)
        $versions = @(
            $tree |
                Where-Object {
                    $_.Name -ieq 'msedgewebview2.exe' -and
                    -not [string]::IsNullOrWhiteSpace($_.ExecutablePath)
                } |
                ForEach-Object {
                    (Get-Item -LiteralPath $_.ExecutablePath).VersionInfo.ProductVersion
                } |
                Sort-Object -Unique
        )
        if ($versions.Count -eq 1) {
            return $versions[0]
        }
        if ($versions.Count -gt 1) {
            break
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTimeOffset]::UtcNow -lt $deadline)
    if ($versions.Count -ne 1) {
        throw "Expected one WebView2 version in the installed process tree; found $($versions -join ',')."
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
    param(
        [Parameter(Mandatory = $true)][int]$RootProcessId,
        [Parameter(Mandatory = $true)][hashtable]$CpuTracker
    )

    $records = @(Get-CimInstance Win32_Process)
    $treeRecords = @(Get-ProcessTreeRecords `
        -Records $records `
        -RootProcessId $RootProcessId)

    $currentCpu = @{}
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
            $createdAt = ([DateTimeOffset]$record.CreationDate).UtcDateTime.Ticks
            $identity = "${processId}:$createdAt"
            $currentSeconds = [double]$process.TotalProcessorTime.TotalSeconds
            $currentCpu[$identity] = $currentSeconds
            if ($CpuTracker.initialized) {
                if ($CpuTracker.previous.ContainsKey($identity)) {
                    $CpuTracker.cumulativeSeconds += [Math]::Max(
                        0,
                        $currentSeconds - [double]$CpuTracker.previous[$identity]
                    )
                }
                else {
                    $CpuTracker.cumulativeSeconds += $currentSeconds
                }
            }
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
    $CpuTracker.previous = $currentCpu
    $CpuTracker.initialized = $true

    return [PSCustomObject]@{
        CpuSeconds = [double]$CpuTracker.cumulativeSeconds
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
