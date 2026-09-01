$ErrorActionPreference = 'Stop'
$executable = '__EXECUTABLE_PATH__'
$projectPath = '__PROJECT_PATH__'
$resultPath = '__RESULT_PATH__'
$controlPath = '__CONTROL_PATH__'
$installRoot = '__INSTALL_ROOT__'
$projectReadyTimeoutSeconds = __PROJECT_READY_TIMEOUT_SECONDS__
$process = $null

__SECURITY_PREAMBLE__

if (-not ('Mendimaru.ProcessSecurity' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Security.Principal;

namespace Mendimaru {
    public static class ProcessSecurity {
        private const uint QueryLimitedInformation = 0x1000;
        private const uint TokenQuery = 0x0008;

        [StructLayout(LayoutKind.Sequential)]
        private struct ProcessBasicInformation {
            public IntPtr Reserved1;
            public IntPtr PebBaseAddress;
            public IntPtr Reserved2_0;
            public IntPtr Reserved2_1;
            public IntPtr UniqueProcessId;
            public IntPtr InheritedFromUniqueProcessId;
        }

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);

        [DllImport("advapi32.dll", SetLastError = true)]
        private static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);

        [DllImport("ntdll.dll")]
        private static extern int NtQueryInformationProcess(
            IntPtr process,
            int informationClass,
            ref ProcessBasicInformation information,
            uint informationLength,
            out uint returnLength
        );

        [DllImport("kernel32.dll")]
        private static extern bool CloseHandle(IntPtr handle);

        public static bool IsCurrentUser(int processId) {
            IntPtr process = OpenProcess(QueryLimitedInformation, false, (uint)processId);
            if (process == IntPtr.Zero) return false;
            IntPtr token = IntPtr.Zero;
            try {
                if (!OpenProcessToken(process, TokenQuery, out token) || token == IntPtr.Zero) {
                    return false;
                }
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

        public static int ParentProcessId(int processId) {
            IntPtr process = OpenProcess(QueryLimitedInformation, false, (uint)processId);
            if (process == IntPtr.Zero) return 0;
            try {
                ProcessBasicInformation information = new ProcessBasicInformation();
                uint returned;
                int status = NtQueryInformationProcess(
                    process,
                    0,
                    ref information,
                    (uint)Marshal.SizeOf(typeof(ProcessBasicInformation)),
                    out returned
                );
                long parent = information.InheritedFromUniqueProcessId.ToInt64();
                return status == 0 && parent > 0 && parent <= Int32.MaxValue ? (int)parent : 0;
            } catch {
                return 0;
            } finally {
                CloseHandle(process);
            }
        }
    }
}
'@
}

function Write-LaunchResult {
    param(
        [string]$State,
        [string]$Message,
        $ExitCode,
        $ExecutablePath,
        $ErrorMessage,
        $Sessions
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        exitCode = $ExitCode
        executablePath = $ExecutablePath
        error = $ErrorMessage
        timestamp = (Get-Date).ToString('o')
        sessions = @($Sessions)
    }
    Write-MendimaruReport $payload
}

function Get-StudioProcesses {
    $matches = New-Object 'System.Collections.Generic.List[object]'
    $candidates = @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue)
    foreach ($candidate in $candidates) {
        try {
            if (-not [Mendimaru.ProcessSecurity]::IsCurrentUser([int]$candidate.Id)) {
                continue
            }
            $candidate.Refresh()
            $candidatePath = [string]$candidate.Path
            if ([string]::IsNullOrWhiteSpace($candidatePath) -or
                -not $candidatePath.Equals(
                    $executable,
                    [StringComparison]::OrdinalIgnoreCase
                )) {
                continue
            }
            $matches.Add([pscustomobject]@{
                Process = $candidate
                ParentProcessId = [Mendimaru.ProcessSecurity]::ParentProcessId([int]$candidate.Id)
            })
        } catch {
            continue
        }
    }
    return $matches.ToArray()
}

function Get-StudioProcessRecord {
    param([int]$ProcessId)

    try {
        $candidate = Get-Process -Id $ProcessId -ErrorAction Stop
        if (-not [Mendimaru.ProcessSecurity]::IsCurrentUser($ProcessId)) {
            return $null
        }
        $candidate.Refresh()
        $candidatePath = [string]$candidate.Path
        if ([string]::IsNullOrWhiteSpace($candidatePath) -or
            -not $candidatePath.Equals(
                $executable,
                [StringComparison]::OrdinalIgnoreCase
            )) {
            return $null
        }
        return [pscustomobject]@{
            Process = $candidate
            ParentProcessId = [Mendimaru.ProcessSecurity]::ParentProcessId($ProcessId)
        }
    } catch {
        return $null
    }
}

function Get-ReadyStudioProcess {
    param($Baseline, [int]$LaunchProcessId)

    $direct = Get-StudioProcessRecord $LaunchProcessId
    if ($null -ne $direct) {
        $candidate = $direct.Process
        $identity = '{0}-{1}' -f @(
            $candidate.Id,
            $candidate.StartTime.ToUniversalTime().Ticks
        )
        if (-not $Baseline.Contains($identity) -and
            $candidate.MainWindowHandle -ne [IntPtr]::Zero) {
            return $candidate
        }
    }

    foreach ($record in (@(Get-StudioProcesses) | Sort-Object { $_.Process.StartTime } -Descending)) {
        $candidate = $record.Process
        $candidate.Refresh()
        $identity = '{0}-{1}' -f @(
            $candidate.Id,
            $candidate.StartTime.ToUniversalTime().Ticks
        )
        if (-not $Baseline.Contains($identity) -and
            ($candidate.Id -eq $LaunchProcessId -or
                $record.ParentProcessId -eq $LaunchProcessId) -and
            $candidate.MainWindowHandle -ne [IntPtr]::Zero) {
            return $candidate
        }
    }
    return $null
}

try {
    Write-LaunchResult 'starting' 'Studio Pro is starting.' $null $executable $null @()
    if (-not [string]::IsNullOrWhiteSpace($projectPath)) {
        if (-not [IO.Path]::GetExtension($projectPath).Equals(
            '.mpr',
            [StringComparison]::OrdinalIgnoreCase
        )) {
            throw 'MENDIMARU_PROJECT_INVALID'
        }
        $projectDeadline = (Get-Date).AddSeconds($projectReadyTimeoutSeconds)
        $projectReady = $false
        do {
            try {
                if (Test-Path -LiteralPath $projectPath -PathType Leaf) {
                    $projectItem = Get-Item -LiteralPath $projectPath -Force -ErrorAction Stop
                    $projectReady = -not $projectItem.PSIsContainer
                }
            } catch {
                $projectReady = $false
            }
            if (-not $projectReady) {
                Start-Sleep -Milliseconds 250
            }
        } while (-not $projectReady -and (Get-Date) -lt $projectDeadline)
        if (-not $projectReady) {
            throw 'MENDIMARU_PROJECT_NOT_READY'
        }
    }
    $baseline = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::Ordinal
    )
    foreach ($existing in @(Get-StudioProcesses)) {
        $existingProcess = $existing.Process
        $null = $baseline.Add(('{0}-{1}' -f @(
            $existingProcess.Id,
            $existingProcess.StartTime.ToUniversalTime().Ticks
        )))
    }
    $null = Assert-MendimaruTrustedExecutable -Path $executable -Root $installRoot
    if ([string]::IsNullOrWhiteSpace($projectPath)) {
        $process = Start-Process -FilePath $executable -PassThru
    } else {
        $quotedProjectPath = '"' + $projectPath + '"'
        $process = Start-Process -FilePath $executable -ArgumentList $quotedProjectPath -PassThru
    }

    $process.Refresh()
    $launchedSessionId = 'studio-{0}-{1}' -f @(
        $process.Id,
        $process.StartTime.ToUniversalTime().Ticks
    )
    $launchedProjectName = if ([string]::IsNullOrWhiteSpace($projectPath)) {
        $null
    } else {
        [IO.Path]::GetFileNameWithoutExtension($projectPath)
    }
    $launchedSession = [ordered]@{
        sessionId = $launchedSessionId
        version = '__VERSION__'
        processId = [int]$process.Id
        startedAt = $process.StartTime.ToUniversalTime().ToString('o')
        projectName = $launchedProjectName
        hasWindow = $false
    }
    Write-LaunchResult 'running' 'Studio Pro process is running.' $null $executable $null @($launchedSession)

    $handoffDeadline = (Get-Date).AddSeconds(15)
    $deadline = (Get-Date).AddMinutes(4)
    $readyProcess = $null
    do {
        $readyProcess = Get-ReadyStudioProcess $baseline $process.Id
        if ($null -ne $readyProcess) {
            break
        }
        if ($process.HasExited -and $null -eq $readyProcess -and (Get-Date) -ge $handoffDeadline) {
            throw "MENDIMARU_STUDIO_EXITED_BEFORE_WINDOW:$($process.ExitCode)"
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)

    if ($null -eq $readyProcess) {
        throw 'MENDIMARU_STUDIO_WINDOW_TIMEOUT'
    }

    # Give FreeRDP time to publish the confirmed Windows handle as a local
    # RemoteApp window before the Linux-side launch button is enabled again.
    Start-Sleep -Milliseconds 750
    $readyProcess.Refresh()
    $sessionId = 'studio-{0}-{1}' -f @(
        $readyProcess.Id,
        $readyProcess.StartTime.ToUniversalTime().Ticks
    )
    $projectName = if ([string]::IsNullOrWhiteSpace($projectPath)) {
        $null
    } else {
        [IO.Path]::GetFileNameWithoutExtension($projectPath)
    }
    $session = [ordered]@{
        sessionId = $sessionId
        version = '__VERSION__'
        processId = [int]$readyProcess.Id
        startedAt = $readyProcess.StartTime.ToUniversalTime().ToString('o')
        projectName = $projectName
        hasWindow = $true
    }
    Write-LaunchResult 'succeeded' 'Studio Pro window is ready.' $null $executable $null @($session)

    # Keep the RemoteApp host bound to this exact PID and start time. Stop
    # requests use this same connection so a second RDP client cannot replace
    # it or expose the Windows RemoteApp selection window.
    $readyProcessId = [int]$readyProcess.Id
    $readyStartedTicks = [long]$readyProcess.StartTime.ToUniversalTime().Ticks
    $lastControlSequence = [long]0
    while ($true) {
        try {
            $current = Get-Process -Id $readyProcessId -ErrorAction Stop
            $current.Refresh()
            if ($current.StartTime.ToUniversalTime().Ticks -ne $readyStartedTicks) {
                break
            }
        } catch {
            break
        }
        if (Test-Path -LiteralPath $controlPath) {
            try {
                $sequence = Read-MendimaruStudioStopRequest `
                    -Path $controlPath `
                    -ExpectedSessionId $sessionId `
                    -ExpectedProcessId $readyProcessId `
                    -ExpectedStartedTicks $readyStartedTicks `
                    -PreviousSequence $lastControlSequence
                $lastControlSequence = $sequence
                Remove-Item -LiteralPath $controlPath -Force -ErrorAction Stop
                $current.Refresh()
                if ($current.StartTime.ToUniversalTime().Ticks -ne $readyStartedTicks) {
                    break
                }
                if ($current.MainWindowHandle -ne [IntPtr]::Zero) {
                    $null = $current.CloseMainWindow()
                }
            } catch {
                # An invalid or tampered request never closes Studio Pro. The
                # host times out while this authenticated launch stays alive.
            }
        }
        Start-Sleep -Milliseconds 500
    }
    Write-LaunchResult 'succeeded' 'Studio Pro session closed.' $null $executable $null @()
    exit 0
} catch {
    $exitCode = if ($null -ne $process -and $process.HasExited) { [int]$process.ExitCode } else { $null }
    if ($null -ne $process -and -not $process.HasExited) {
        try {
            $record = Get-StudioProcessRecord ([int]$process.Id)
            if ($null -ne $record -and $record.Process.MainWindowHandle -ne [IntPtr]::Zero) {
                $null = $record.Process.CloseMainWindow()
            }
        } catch {
            # The authenticated host-side abort path owns forced cleanup.
        }
    }
    Write-LaunchResult 'failed' 'Studio Pro failed to start.' $exitCode $executable $_.Exception.Message @()
    exit 1
}
