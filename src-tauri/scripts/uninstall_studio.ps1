$ErrorActionPreference = 'Stop'
$dataRoot = '__DATA_ROOT__'
$installRoot = '__INSTALL_ROOT__'
$version = '__VERSION__'
$resultPath = '__RESULT_PATH__'
$process = $null

__SECURITY_PREAMBLE__

function Write-UninstallResult {
    param(
        [string]$State,
        [string]$Message,
        $ExitCode,
        $ErrorMessage
    )

    $payload = [ordered]@{
        state = $State
        message = $Message
        exitCode = $ExitCode
        executablePath = $null
        error = $ErrorMessage
        timestamp = (Get-Date).ToString('o')
    }
    Write-MendimaruReport $payload
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

function Get-RunningStudioPro {
    param([string]$ExecutablePath)

    return @(Get-Process -Name 'studiopro' -ErrorAction SilentlyContinue | Where-Object {
        try {
            $_.Path -and $_.Path.Equals($ExecutablePath, [System.StringComparison]::OrdinalIgnoreCase)
        } catch {
            $false
        }
    })
}

try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal] $identity
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'MENDIMARU_ADMIN_REQUIRED'
    }

    $studioPro = Find-StudioPro
    if ($null -ne $studioPro) {
        $null = Assert-MendimaruTrustedExecutable -Path $studioPro -Root $installRoot
        if (@(Get-RunningStudioPro $studioPro).Count -gt 0) {
            throw 'MENDIMARU_STUDIO_RUNNING'
        }
    }

    $folder = Get-ChildItem -LiteralPath $dataRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $version -or $_.Name.StartsWith("$version.") } |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $folder) {
        throw "MENDIMARU_UNINSTALL_METADATA_MISSING:$version"
    }

    $uninstaller = Join-Path $folder.FullName 'uninst\unins000.exe'
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        throw "MENDIMARU_UNINSTALLER_NOT_FOUND:$uninstaller"
    }
    $null = Assert-MendimaruTrustedExecutable -Path $uninstaller -Root $dataRoot

    Write-UninstallResult 'running' 'Studio Pro uninstaller is running.' $null $null
    $null = Assert-MendimaruTrustedExecutable -Path $uninstaller -Root $dataRoot
    $process = Start-Process -FilePath $uninstaller -ArgumentList @('/SILENT') -Wait -PassThru
    $exitCode = [int]$process.ExitCode
    if (@(0, 1641, 3010) -notcontains $exitCode) {
        throw "MENDIMARU_UNINSTALLER_EXIT_CODE:$exitCode"
    }

    $deadline = (Get-Date).AddMinutes(3)
    $studioPro = Find-StudioPro
    while ($null -ne $studioPro -and (Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        $studioPro = Find-StudioPro
    }
    if ($null -ne $studioPro) {
        throw "MENDIMARU_UNINSTALL_STILL_EXISTS:$studioPro"
    }

    Write-UninstallResult 'succeeded' 'Studio Pro uninstall completed.' $exitCode $null
    exit 0
} catch {
    $exitCode = if ($null -ne $process) { [int]$process.ExitCode } else { $null }
    Write-UninstallResult 'failed' 'Studio Pro uninstall failed.' $exitCode $_.Exception.Message
    exit 1
}
