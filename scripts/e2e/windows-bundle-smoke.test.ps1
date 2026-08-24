$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptPath = Join-Path $PSScriptRoot 'windows-bundle-smoke.ps1'
. $scriptPath `
    -InstallerKind nsis `
    -InstallerPath (Join-Path $PSScriptRoot 'unused.exe') `
    -HelpersOnly

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Equal {
    param(
        [AllowNull()]$Actual,
        [AllowNull()]$Expected,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if ($Actual -cne $Expected) {
        throw "$Message Expected '$Expected', received '$Actual'."
    }
}

Assert-True `
    -Condition (-not (Test-MendimaruUninstallEntry -Entry ([PSCustomObject]@{}))) `
    -Message 'A sparse uninstall entry without DisplayName must be ignored.'
Assert-True `
    -Condition (-not (Test-MendimaruUninstallEntry -Entry ([PSCustomObject]@{
        DisplayName = $null
    }))) `
    -Message 'A null DisplayName must be ignored.'
Assert-True `
    -Condition (Test-MendimaruUninstallEntry -Entry ([PSCustomObject]@{
        DisplayName = 'MENDIMARU'
    })) `
    -Message 'The Mendimaru display name match must be case-insensitive.'
Assert-True `
    -Condition (-not (Test-MendimaruUninstallEntry -Entry ([PSCustomObject]@{
        DisplayName = 'Another application'
    }))) `
    -Message 'An unrelated uninstall entry must be ignored.'

$rootCreatedAt = [DateTimeOffset]::UtcNow
$processRecords = @(
    [PSCustomObject]@{
        ProcessId = 400
        ParentProcessId = 10
        CreationDate = $rootCreatedAt
    }
    [PSCustomObject]@{
        ProcessId = 401
        ParentProcessId = 400
        CreationDate = $rootCreatedAt.AddMilliseconds(10)
    }
    [PSCustomObject]@{
        ProcessId = 402
        ParentProcessId = 401
        CreationDate = $rootCreatedAt.AddMilliseconds(20)
    }
    [PSCustomObject]@{
        ProcessId = 410
        ParentProcessId = 400
        CreationDate = $rootCreatedAt.AddMinutes(-10)
    }
    [PSCustomObject]@{
        ProcessId = 411
        ParentProcessId = 410
        CreationDate = $rootCreatedAt.AddMinutes(-9)
    }
    [PSCustomObject]@{
        ProcessId = 420
        ParentProcessId = 10
        CreationDate = $rootCreatedAt.AddMilliseconds(30)
    }
)
$selectedProcessIds = @(
    Get-ProcessTreeRecords -Records $processRecords -RootProcessId 400 |
        ForEach-Object { [int]$_.ProcessId }
)
Assert-Equal `
    -Actual ($selectedProcessIds -join ',') `
    -Expected '400,401,402' `
    -Message 'A reused root PID must not adopt older orphaned processes or their descendants.'

$temporary = Join-Path ([IO.Path]::GetTempPath()) "mendimaru-bundle-helper-$([Guid]::NewGuid().ToString('N'))"
$originalProgramFiles = $env:ProgramFiles
$originalLocalAppData = $env:LOCALAPPDATA
New-Item -ItemType Directory -Path $temporary -Force | Out-Null

try {
    $env:ProgramFiles = Join-Path $temporary 'Program Files'
    $env:LOCALAPPDATA = Join-Path $temporary 'Local AppData'
    New-Item -ItemType Directory -Path $env:ProgramFiles -Force | Out-Null
    New-Item -ItemType Directory -Path $env:LOCALAPPDATA -Force | Out-Null

    $quotedRoot = Join-Path $temporary 'Quoted NSIS Install'
    $quotedExecutable = Join-Path $quotedRoot 'mendimaru.exe'
    New-Item -ItemType Directory -Path $quotedRoot -Force | Out-Null
    New-Item -ItemType File -Path $quotedExecutable -Force | Out-Null
    $resolved = Resolve-MendimaruExecutable -Entry ([PSCustomObject]@{
        InstallLocation = "`"$quotedRoot`""
    })
    Assert-Equal `
        -Actual $resolved `
        -Expected ([IO.Path]::GetFullPath($quotedExecutable)) `
        -Message 'A quoted NSIS InstallLocation must resolve to the installed executable.'
    Remove-Item -LiteralPath $quotedExecutable -Force

    $iconRoot = Join-Path $temporary 'Icon Install'
    $iconExecutable = Join-Path $iconRoot 'mendimaru.exe'
    New-Item -ItemType Directory -Path $iconRoot -Force | Out-Null
    New-Item -ItemType File -Path $iconExecutable -Force | Out-Null
    $resolved = Resolve-MendimaruExecutable -Entry ([PSCustomObject]@{
        DisplayIcon = "`"$iconExecutable`",0"
    })
    Assert-Equal `
        -Actual $resolved `
        -Expected ([IO.Path]::GetFullPath($iconExecutable)) `
        -Message 'A quoted DisplayIcon with an icon index must resolve to the executable.'
    Remove-Item -LiteralPath $iconExecutable -Force

    $fallbackExecutable = Join-Path $env:LOCALAPPDATA 'Programs\mendimaru\mendimaru.exe'
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($fallbackExecutable)) -Force |
        Out-Null
    New-Item -ItemType File -Path $fallbackExecutable -Force | Out-Null
    $resolved = Resolve-MendimaruExecutable -Entry ([PSCustomObject]@{})
    Assert-Equal `
        -Actual $resolved `
        -Expected ([IO.Path]::GetFullPath($fallbackExecutable)) `
        -Message 'A sparse uninstall entry must use a standard installation fallback.'
    Remove-Item -LiteralPath $fallbackExecutable -Force

    $missingFailed = $false
    try {
        Resolve-MendimaruExecutable -Entry ([PSCustomObject]@{
            InstallLocation = '   '
            DisplayIcon = $null
        }) | Out-Null
    }
    catch {
        $missingFailed = $_.Exception.Message -like '*found 0*'
    }
    Assert-True `
        -Condition $missingFailed `
        -Message 'Resolution must fail closed when no candidate executable exists.'

    $firstRoot = Join-Path $temporary 'Duplicate One'
    $secondRoot = Join-Path $temporary 'Duplicate Two'
    $firstExecutable = Join-Path $firstRoot 'mendimaru.exe'
    $secondExecutable = Join-Path $secondRoot 'mendimaru.exe'
    New-Item -ItemType Directory -Path $firstRoot, $secondRoot -Force | Out-Null
    New-Item -ItemType File -Path $firstExecutable, $secondExecutable -Force | Out-Null
    $duplicateFailed = $false
    try {
        Resolve-MendimaruExecutable -Entry ([PSCustomObject]@{
            InstallLocation = $firstRoot
            DisplayIcon = $secondExecutable
        }) | Out-Null
    }
    catch {
        $duplicateFailed = $_.Exception.Message -like '*found 2*'
    }
    Assert-True `
        -Condition $duplicateFailed `
        -Message 'Resolution must fail when registry metadata identifies multiple executables.'
}
finally {
    $env:ProgramFiles = $originalProgramFiles
    $env:LOCALAPPDATA = $originalLocalAppData
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output 'Windows bundle smoke helper tests passed.'
