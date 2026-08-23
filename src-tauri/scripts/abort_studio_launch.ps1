$ErrorActionPreference = 'Stop'
$executable = '__EXECUTABLE_PATH__'
$resultPath = '__RESULT_PATH__'
$installRoot = '__INSTALL_ROOT__'
$targetProcessId = [int]__TARGET_PROCESS_ID__
$targetStartedTicks = [long]__TARGET_STARTED_TICKS__

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
    }
}
'@
}

function Write-AbortResult {
    param([string]$State, [string]$Message, $ErrorMessage)

    $payload = [ordered]@{
        state = $State
        message = $Message
        percentage = $null
        estimated = $false
        timestamp = (Get-Date).ToString('o')
        exitCode = $null
        executablePath = $executable
        error = $ErrorMessage
        sessions = @()
    }
    Write-MendimaruReport $payload
}

function Get-ExactLaunchedProcess {
    try {
        $candidate = Get-Process -Id $targetProcessId -ErrorAction Stop
        if (-not [Mendimaru.ProcessSecurity]::IsCurrentUser($targetProcessId)) {
            return $null
        }
        $candidate.Refresh()
        if (-not ([string]$candidate.Path).Equals(
                $executable,
                [StringComparison]::OrdinalIgnoreCase
            ) -or
            $candidate.StartTime.ToUniversalTime().Ticks -ne $targetStartedTicks) {
            return $null
        }
        return $candidate
    } catch {
        return $null
    }
}

try {
    $null = Assert-MendimaruTrustedExecutable -Path $executable -Root $installRoot
    $process = Get-ExactLaunchedProcess
    if ($null -ne $process) {
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) {
            $null = $process.CloseMainWindow()
        }
        $gracefulDeadline = (Get-Date).AddSeconds(10)
        while ($null -ne (Get-ExactLaunchedProcess) -and (Get-Date) -lt $gracefulDeadline) {
            Start-Sleep -Milliseconds 250
        }
        $process = Get-ExactLaunchedProcess
        if ($null -ne $process) {
            Stop-Process -Id $targetProcessId -Force -ErrorAction Stop
            $forcedDeadline = (Get-Date).AddSeconds(15)
            while ($null -ne (Get-ExactLaunchedProcess) -and (Get-Date) -lt $forcedDeadline) {
                Start-Sleep -Milliseconds 250
            }
        }
    }
    if ($null -ne (Get-ExactLaunchedProcess)) {
        throw 'MENDIMARU_STUDIO_ABORT_PENDING'
    }
    Write-AbortResult 'succeeded' 'Incomplete Studio Pro launch cleaned up.' $null
    exit 0
} catch {
    Write-AbortResult 'failed' 'Incomplete Studio Pro launch cleanup failed.' 'MENDIMARU_STUDIO_ABORT_FAILED'
    exit 1
}
