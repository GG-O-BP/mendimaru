$ErrorActionPreference = 'Stop'
$installer = '__INSTALLER_PATH__'
$resultPath = '__RESULT_PATH__'
$installRoot = '__INSTALL_ROOT__'
$version = '__VERSION__'
$process = $null
$localInstaller = $null
$scriptExitCode = 0

function Write-InstallResult {
    param(
        [string]$State,
        [string]$Message,
        $Percentage,
        [bool]$Estimated,
        $ExitCode,
        $ExecutablePath,
        $ErrorMessage
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        percentage = $Percentage
        estimated = $Estimated
        exitCode = $ExitCode
        executablePath = $ExecutablePath
        error = $ErrorMessage
        timestamp = (Get-Date).ToString('o')
    }
    $temporaryPath = "$resultPath.tmp"
    $serialized = $payload | ConvertTo-Json -Compress
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try {
            $serialized | Set-Content -LiteralPath $temporaryPath -Encoding UTF8
            Move-Item -LiteralPath $temporaryPath -Destination $resultPath -Force
            return
        } catch {
            Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
            if ($attempt -eq 19) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

if (-not ('Mendimaru.NativeProgressReader' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace Mendimaru {
    public static class NativeProgressReader {
        private const uint WM_USER = 0x0400;
        private const uint PBM_GETRANGE = WM_USER + 7;
        private const uint PBM_GETPOS = WM_USER + 8;
        private const uint SMTO_ABORTIFHUNG = 0x0002;
        private static HashSet<int> processIds;
        private static double bestProgress;

        private delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

        [DllImport("user32.dll")]
        private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);

        [DllImport("user32.dll")]
        private static extern bool EnumChildWindows(
            IntPtr parent,
            EnumWindowsProc callback,
            IntPtr parameter
        );

        [DllImport("user32.dll")]
        private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetClassName(
            IntPtr window,
            StringBuilder className,
            int maximumCount
        );

        [DllImport("user32.dll")]
        private static extern bool IsWindowVisible(IntPtr window);

        [DllImport("user32.dll", SetLastError = true)]
        private static extern IntPtr SendMessageTimeout(
            IntPtr window,
            uint message,
            UIntPtr wordParameter,
            IntPtr longParameter,
            uint flags,
            uint timeoutMilliseconds,
            out UIntPtr result
        );

        public static double Read(int[] candidateProcessIds) {
            processIds = new HashSet<int>(candidateProcessIds);
            bestProgress = -1.0;
            EnumWindows(new EnumWindowsProc(InspectTopLevelWindow), IntPtr.Zero);
            return bestProgress;
        }

        private static bool InspectTopLevelWindow(IntPtr window, IntPtr parameter) {
            uint processId;
            GetWindowThreadProcessId(window, out processId);
            if (!processIds.Contains((int)processId)) {
                return true;
            }

            InspectProgressControl(window);
            EnumChildWindows(window, new EnumWindowsProc(InspectChildWindow), IntPtr.Zero);
            return true;
        }

        private static bool InspectChildWindow(IntPtr window, IntPtr parameter) {
            InspectProgressControl(window);
            return true;
        }

        private static void InspectProgressControl(IntPtr window) {
            if (!IsWindowVisible(window)) {
                return;
            }
            StringBuilder className = new StringBuilder(128);
            if (GetClassName(window, className, className.Capacity) == 0) {
                return;
            }
            string nativeClass = className.ToString();
            if (!String.Equals(
                    nativeClass,
                    "msctls_progress32",
                    StringComparison.OrdinalIgnoreCase
                ) && !String.Equals(
                    nativeClass,
                    "TNewProgressBar",
                    StringComparison.OrdinalIgnoreCase
                )) {
                return;
            }

            int minimum = ReadMessage(window, PBM_GETRANGE, new UIntPtr(1));
            int maximum = ReadMessage(window, PBM_GETRANGE, UIntPtr.Zero);
            int position = ReadMessage(window, PBM_GETPOS, UIntPtr.Zero);
            if (minimum < 0 || maximum <= minimum || position < minimum) {
                return;
            }

            double value = Math.Min(
                1.0,
                Math.Max(0.0, (double)(position - minimum) / (maximum - minimum))
            );
            if (value > bestProgress) {
                bestProgress = value;
            }
        }

        private static int ReadMessage(IntPtr window, uint message, UIntPtr wordParameter) {
            UIntPtr result;
            IntPtr succeeded = SendMessageTimeout(
                window,
                message,
                wordParameter,
                IntPtr.Zero,
                SMTO_ABORTIFHUNG,
                250,
                out result
            );
            return succeeded == IntPtr.Zero ? -1 : unchecked((int)result.ToUInt64());
        }
    }
}
'@
}

function Get-DescendantProcessIds {
    param([int]$RootProcessId)

    $processIds = New-Object 'System.Collections.Generic.HashSet[int]'
    $null = $processIds.Add($RootProcessId)
    $processes = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
    $changed = $true
    while ($changed) {
        $changed = $false
        foreach ($candidate in $processes) {
            if ($processIds.Contains([int]$candidate.ParentProcessId) -and
                $processIds.Add([int]$candidate.ProcessId)) {
                $changed = $true
            }
        }
    }
    return [int[]]@($processIds)
}

function Get-InstallerProgress {
    param([int]$RootProcessId)

    $processIds = @(Get-DescendantProcessIds $RootProcessId)
    if ($processIds.Count -eq 0) { return $null }
    $value = [Mendimaru.NativeProgressReader]::Read([int[]]$processIds)
    if ($value -lt 0) { return $null }
    return [double]$value
}

function Copy-InstallerWithProgress {
    param(
        [string]$Source,
        [string]$Destination
    )

    $sourceInfo = Get-Item -LiteralPath $Source
    $destinationInfo = Get-Item -LiteralPath $Destination -ErrorAction SilentlyContinue
    if ($null -ne $destinationInfo -and $destinationInfo.Length -eq $sourceInfo.Length) {
        Write-InstallResult 'staging' 'Installer is ready in Windows.' 100 $false $null $null $null
        return
    }

    $inputStream = [System.IO.File]::Open(
        $sourceInfo.FullName,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read
    )
    try {
        $outputStream = [System.IO.File]::Open(
            $Destination,
            [System.IO.FileMode]::Create,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        try {
            $buffer = New-Object byte[] 4194304
            [long]$copied = 0
            $lastPercentage = -1
            while (($read = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $outputStream.Write($buffer, 0, $read)
                $copied += $read
                $percentage = [int][Math]::Floor(100.0 * $copied / $sourceInfo.Length)
                if ($percentage -ne $lastPercentage) {
                    Write-InstallResult 'staging' 'Copying installer into Windows.' $percentage $false $null $null $null
                    $lastPercentage = $percentage
                }
            }
            $outputStream.Flush()
        } finally {
            $outputStream.Dispose()
        }
    } finally {
        $inputStream.Dispose()
    }
}

function Find-StudioPro {
    $folders = Get-ChildItem -LiteralPath $installRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $version -or $_.Name.StartsWith("$version.") } |
        Sort-Object Name -Descending

    foreach ($folder in $folders) {
        $candidate = Join-Path $folder.FullName 'modeler\studiopro.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    return $null
}

try {
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw "MENDIMARU_INSTALLER_NOT_FOUND:$installer"
    }

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal] $identity
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'MENDIMARU_ADMIN_REQUIRED'
    }

    # Executing an installer directly from the host UNC share can block on an
    # invisible "Open File - Security Warning" dialog in RemoteApp. Stage it
    # locally and remove any downloaded-file zone marker before launching it.
    $sourceInstaller = Get-Item -LiteralPath $installer
    $stagingDirectory = Join-Path $env:ProgramData 'Mendimaru\Installers'
    New-Item -ItemType Directory -Path $stagingDirectory -Force | Out-Null
    $localInstaller = Join-Path $stagingDirectory $sourceInstaller.Name
    Write-InstallResult 'staging' 'Preparing the installer in Windows.' 0 $false $null $null $null
    Copy-InstallerWithProgress $installer $localInstaller
    Unblock-File -LiteralPath $localInstaller -ErrorAction SilentlyContinue

    $logDirectory = Join-Path $env:ProgramData 'Mendimaru\Logs'
    New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null
    $installLog = Join-Path $logDirectory ("install-$version-{0}.log" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
    $logArgument = '/LOG="' + $installLog + '"'
    Write-InstallResult 'installing' 'Studio Pro installer is running.' 0 $true $null $null $null
    $process = Start-Process -FilePath $localInstaller -ArgumentList @(
        '/SP-',
        '/SILENT',
        '/SUPPRESSMSGBOXES',
        '/NOCANCEL',
        '/NORESTART',
        $logArgument
    ) -PassThru

    $installerStartedAt = Get-Date
    $bestPhasePercentage = 0.0
    $bestPhaseEstimated = $true
    $observedNativeProgress = $false
    $lastReportedPercentage = -1
    $lastReportedState = ''
    $lastReportedEstimated = $null
    $lastReportWrittenAt = Get-Date
    while (-not $process.HasExited) {
        $nativeProgress = Get-InstallerProgress $process.Id
        if ($null -ne $nativeProgress) {
            $observedNativeProgress = $true
            $candidatePercentage = [Math]::Min(100.0, [Math]::Max(0.0, $nativeProgress * 100.0))
            $isEstimated = $false
        } elseif ($observedNativeProgress) {
            # The progress window can briefly disappear during installer-owned
            # post-install actions. Preserve the last native value in that case.
            $candidatePercentage = $bestPhasePercentage
            $isEstimated = $bestPhaseEstimated
        } else {
            # Inno Setup does not expose a supported cross-process callback.
            # Until its native progress control appears, use a deliberately
            # slowing curve calibrated against live LTS installs and cap it.
            $elapsedSeconds = ((Get-Date) - $installerStartedAt).TotalSeconds
            $candidatePercentage = [Math]::Min(
                95.0,
                95.0 * (1.0 - [Math]::Exp(-$elapsedSeconds / 105.0))
            )
            $isEstimated = $true
        }

        if ($candidatePercentage -gt $bestPhasePercentage) {
            $bestPhasePercentage = $candidatePercentage
            $bestPhaseEstimated = $isEstimated
        } elseif (-not $isEstimated -and $candidatePercentage -ge $bestPhasePercentage) {
            $bestPhaseEstimated = $false
        }
        $isEstimated = $bestPhaseEstimated
        $reportState = if ($observedNativeProgress -and $bestPhasePercentage -ge 99.9) {
            'finalizing'
        } else {
            'installing'
        }
        $roundedPercentage = [int][Math]::Floor($bestPhasePercentage)
        if ($roundedPercentage -ne $lastReportedPercentage -or
            $reportState -ne $lastReportedState -or
            $isEstimated -ne $lastReportedEstimated -or
            ((Get-Date) - $lastReportWrittenAt).TotalSeconds -ge 5) {
            $reportMessage = if ($reportState -eq 'finalizing') {
                'Completing installer actions.'
            } elseif ($isEstimated) {
                'Installing Studio Pro (estimated progress).'
            } else {
                'Installing Studio Pro.'
            }
            Write-InstallResult $reportState $reportMessage $bestPhasePercentage $isEstimated $null $null $null
            $lastReportedPercentage = $roundedPercentage
            $lastReportedState = $reportState
            $lastReportedEstimated = $isEstimated
            $lastReportWrittenAt = Get-Date
        }

        Start-Sleep -Milliseconds 750
        $process.Refresh()
    }
    $process.WaitForExit()
    $exitCode = [int]$process.ExitCode
    if (@(0, 1641, 3010) -notcontains $exitCode) {
        throw "MENDIMARU_INSTALLER_EXIT_CODE:$exitCode"
    }

    Write-InstallResult 'verifying' 'Verifying the Studio Pro installation.' 0 $false $exitCode $null $null
    $deadline = (Get-Date).AddMinutes(3)
    $verificationStartedAt = Get-Date
    $lastVerificationPercentage = -1
    $studioPro = $null
    do {
        $studioPro = Find-StudioPro
        if ($null -ne $studioPro) { break }
        $verificationPercentage = [Math]::Min(
            95.0,
            ((Get-Date) - $verificationStartedAt).TotalSeconds / 1.8
        )
        $roundedVerificationPercentage = [int][Math]::Floor($verificationPercentage)
        if ($roundedVerificationPercentage -ne $lastVerificationPercentage) {
            Write-InstallResult 'verifying' 'Verifying the Studio Pro installation.' $verificationPercentage $false $exitCode $null $null
            $lastVerificationPercentage = $roundedVerificationPercentage
        }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)

    if ($null -eq $studioPro) {
        throw "MENDIMARU_STUDIO_NOT_CREATED:$version"
    }

    Write-InstallResult 'verifying' 'Studio Pro installation verified.' 100 $false $exitCode $studioPro $null
    Write-InstallResult 'succeeded' 'Studio Pro installation completed.' 100 $false $exitCode $studioPro $null
} catch {
    $exitCode = if ($null -ne $process -and $process.HasExited) { [int]$process.ExitCode } else { $null }
    Write-InstallResult 'failed' 'Studio Pro installation failed.' $null $false $exitCode $null $_.Exception.Message
    $scriptExitCode = 1
} finally {
    if ($null -ne $localInstaller) {
        Remove-Item -LiteralPath $localInstaller -Force -ErrorAction SilentlyContinue
    }
}
exit $scriptExitCode
