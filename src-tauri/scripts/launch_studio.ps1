$ErrorActionPreference = 'Stop'
$executable = '__EXECUTABLE_PATH__'
$projectPath = '__PROJECT_PATH__'
$resultPath = '__RESULT_PATH__'
$process = $null

function Write-LaunchResult {
    param(
        [string]$State,
        [string]$Message,
        $ExitCode,
        $ExecutablePath,
        $ErrorMessage
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        exitCode = $ExitCode
        executablePath = $ExecutablePath
        error = $ErrorMessage
        timestamp = (Get-Date).ToString('o')
    }
    $temporaryPath = "$resultPath.tmp"
    $payload | ConvertTo-Json -Compress | Set-Content -LiteralPath $temporaryPath -Encoding UTF8
    Move-Item -LiteralPath $temporaryPath -Destination $resultPath -Force
}

function Get-StudioProcesses {
    return @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue | Where-Object {
        try {
            $_.Path -and $_.Path.Equals($executable, [System.StringComparison]::OrdinalIgnoreCase)
        } catch {
            $false
        }
    })
}

function Get-ReadyStudioProcess {
    foreach ($candidate in (@(Get-StudioProcesses) | Sort-Object StartTime -Descending)) {
        $candidate.Refresh()
        if ($candidate.MainWindowHandle -ne [IntPtr]::Zero) {
            return $candidate
        }
    }
    return $null
}

try {
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "MENDIMARU_STUDIO_EXECUTABLE_NOT_FOUND:$executable"
    }

    Write-LaunchResult 'starting' 'Studio Pro is starting.' $null $executable $null
    if ([string]::IsNullOrWhiteSpace($projectPath)) {
        $process = Start-Process -FilePath $executable -PassThru
    } else {
        $quotedProjectPath = '"' + $projectPath + '"'
        $process = Start-Process -FilePath $executable -ArgumentList $quotedProjectPath -PassThru
    }

    $minimumReadyAt = (Get-Date).AddSeconds(2)
    $handoffDeadline = (Get-Date).AddSeconds(15)
    $deadline = (Get-Date).AddMinutes(4)
    $readyProcess = $null
    do {
        $readyProcess = Get-ReadyStudioProcess
        if ($null -ne $readyProcess -and (Get-Date) -ge $minimumReadyAt) {
            break
        }
        if ($process.HasExited -and $null -eq $readyProcess -and (Get-Date) -ge $handoffDeadline) {
            throw "MENDIMARU_STUDIO_EXITED_BEFORE_WINDOW:$($process.ExitCode)"
        }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)

    if ($null -eq $readyProcess) {
        throw 'MENDIMARU_STUDIO_WINDOW_TIMEOUT'
    }

    # Give FreeRDP time to publish the confirmed Windows handle as a local
    # RemoteApp window before the Linux-side launch button is enabled again.
    Start-Sleep -Milliseconds 1200
    Write-LaunchResult 'succeeded' 'Studio Pro window is ready.' $null $executable $null

    # Studio Pro can hand the sign-in window to another studiopro.exe process.
    # Keep the RemoteApp host alive across that handoff and only exit after no
    # process with the selected executable path has existed for 15 seconds.
    $missingSince = $null
    while ($true) {
        $studioProcesses = @(Get-StudioProcesses)
        if ($studioProcesses.Count -gt 0) {
            $missingSince = $null
        } elseif ($null -eq $missingSince) {
            $missingSince = Get-Date
        } elseif (((Get-Date) - $missingSince).TotalSeconds -ge 15) {
            break
        }
        Start-Sleep -Milliseconds 500
    }
    exit 0
} catch {
    $exitCode = if ($null -ne $process -and $process.HasExited) { [int]$process.ExitCode } else { $null }
    Write-LaunchResult 'failed' 'Studio Pro failed to start.' $exitCode $executable $_.Exception.Message
    exit 1
}
