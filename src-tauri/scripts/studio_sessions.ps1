$ErrorActionPreference = 'Stop'
$mode = '__MODE__'
$targetProcessId = [int]__TARGET_PROCESS_ID__
$targetStartedTicks = [long]__TARGET_STARTED_TICKS__
$resultPath = '__RESULT_PATH__'
$installRoot = '__INSTALL_ROOT__'
$knownStudiosJson = [Text.Encoding]::UTF8.GetString(
    [Convert]::FromBase64String('__KNOWN_STUDIOS_BASE64__')
)
$knownStudios = @($knownStudiosJson | ConvertFrom-Json)

__SECURITY_PREAMBLE__

if (-not ('Mendimaru.ProcessSecurity' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
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

function Write-SessionResult {
    param(
        [string]$State,
        [string]$Message,
        $ErrorMessage,
        $Sessions
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        percentage = $null
        estimated = $false
        timestamp = (Get-Date).ToString('o')
        exitCode = $null
        executablePath = $null
        error = $ErrorMessage
        sessions = @($Sessions)
    }
    Write-MendimaruReport $payload
}

function Get-SafeSessionError {
    param([string]$Message)

    if (@(
        'MENDIMARU_STUDIO_SESSION_ENDED',
        'MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE',
        'MENDIMARU_STUDIO_SESSION_CLOSE_PENDING',
        'MENDIMARU_STUDIO_SESSION_CLOSE_REJECTED',
        'MENDIMARU_STUDIO_SESSION_ENUMERATION_FAILED',
        'MENDIMARU_STUDIO_SESSION_REPORT_FAILED'
    ) -contains $Message) {
        return $Message
    }
    return 'MENDIMARU_STUDIO_SESSION_QUERY_FAILED'
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
                $path = if ($match.Groups[1].Success) {
                    $match.Groups[1].Value
                } else {
                    $match.Groups[2].Value
                }
                $name = [IO.Path]::GetFileNameWithoutExtension($path)
            }
        }
    } catch {
        $name = $null
    }
    if ([string]::IsNullOrWhiteSpace($name)) {
        $titleMatch = [regex]::Match(
            [string]$NativeProcess.MainWindowTitle,
            '^(.*?)\s+-\s+Mendix Studio Pro(?:\s|$)'
        )
        if ($titleMatch.Success) { $name = $titleMatch.Groups[1].Value }
    }
    if ([string]::IsNullOrWhiteSpace($name) -or $name.Length -gt 160 -or
        $name.IndexOfAny([char[]]@(0, 10, 13)) -ge 0) {
        return $null
    }
    return $name
}

function Get-StudioSessions {
    $sessions = New-Object 'System.Collections.Generic.List[object]'
    $processes = @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue)
    foreach ($candidate in $processes) {
        if ($sessions.Count -ge 64 -or
            -not [Mendimaru.ProcessSecurity]::IsCurrentUser([int]$candidate.Id)) {
            continue
        }

        try {
            $native = Get-Process -Id ([int]$candidate.Id) -ErrorAction Stop
            $native.Refresh()
            $executablePath = [string]$native.Path
            if ([string]::IsNullOrWhiteSpace($executablePath)) { continue }
            $known = @($knownStudios | Where-Object {
                $_.path.Equals(
                    $executablePath,
                    [StringComparison]::OrdinalIgnoreCase
                )
            } | Select-Object -First 1)
            if ($known.Count -ne 1) { continue }
            $started = $native.StartTime.ToUniversalTime()
            $startedTicks = $started.Ticks
            if ($targetProcessId -gt 0 -and
                ($native.Id -ne $targetProcessId -or $startedTicks -ne $targetStartedTicks)) {
                continue
            }
            $null = Assert-MendimaruTrustedExecutable `
                -Path $executablePath `
                -Root $installRoot
            $sessions.Add([ordered]@{
                sessionId = 'studio-{0}-{1}' -f @($native.Id, $startedTicks)
                version = [string]$known[0].version
                processId = [int]$native.Id
                startedAt = $started.ToString('o')
                projectName = Get-ProjectName $native
                hasWindow = $native.MainWindowHandle -ne [IntPtr]::Zero
            })
        } catch {
            continue
        }
    }
    return $sessions.ToArray()
}

function Get-TargetSession {
    $matches = @(Get-StudioSessions)
    if ($matches.Count -ne 1) {
        throw 'MENDIMARU_STUDIO_SESSION_ENDED'
    }
    return $matches[0]
}

function Get-TargetProcess {
    $session = Get-TargetSession
    $process = Get-Process -Id ([int]$session.processId) -ErrorAction Stop
    $process.Refresh()
    return $process
}

try {
    switch ($mode) {
        'query' {
            try {
                $sessions = @(Get-StudioSessions)
            } catch {
                throw 'MENDIMARU_STUDIO_SESSION_ENUMERATION_FAILED'
            }
            try {
                Write-SessionResult 'succeeded' 'Studio Pro sessions inspected.' $null $sessions
            } catch {
                throw 'MENDIMARU_STUDIO_SESSION_REPORT_FAILED'
            }
            exit 0
        }
        'reconnect' {
            $session = Get-TargetSession
            if (-not $session.hasWindow) {
                throw 'MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE'
            }
            $process = Get-Process -Id ([int]$session.processId) -ErrorAction Stop
            $process.Refresh()
            Write-SessionResult 'succeeded' 'Studio Pro session is ready to reconnect.' $null @($session)
            while ($true) {
                Start-Sleep -Seconds 1
                try {
                    $process.Refresh()
                    if ($process.HasExited -or
                        $process.StartTime.ToUniversalTime().Ticks -ne $targetStartedTicks) {
                        exit 0
                    }
                } catch {
                    exit 0
                }
            }
        }
        'stop' {
            $session = Get-TargetSession
            if (-not $session.hasWindow) {
                throw 'MENDIMARU_STUDIO_SESSION_WINDOW_UNAVAILABLE'
            }
            $process = Get-TargetProcess
            if (-not $process.CloseMainWindow()) {
                throw 'MENDIMARU_STUDIO_SESSION_CLOSE_REJECTED'
            }
            $deadline = (Get-Date).AddSeconds(30)
            do {
                Start-Sleep -Milliseconds 500
                $process.Refresh()
            } while (-not $process.HasExited -and (Get-Date) -lt $deadline)
            if (-not $process.HasExited) {
                throw 'MENDIMARU_STUDIO_SESSION_CLOSE_PENDING'
            }
            Write-SessionResult 'succeeded' 'Studio Pro session closed.' $null @()
            exit 0
        }
        default {
            throw 'MENDIMARU_STUDIO_SESSION_QUERY_FAILED'
        }
    }
} catch {
    $safeError = Get-SafeSessionError $_.Exception.Message
    Write-SessionResult 'failed' 'Studio Pro session operation failed.' $safeError @()
    exit 1
}
