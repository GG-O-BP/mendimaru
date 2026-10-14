# Exercise the production identity probe without acting on a Studio process.
$ErrorActionPreference = 'Stop'
. (Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/scripts/studio_probe.ps1')
function Assert([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }

$current = Get-Process -Id $PID
$ticks = $current.StartTime.ToUniversalTime().Ticks
$observed = Get-MendimaruExactStudioProcess -ExpectedProcessId $PID -ExpectedStartedTicks $ticks
Assert ($observed.Id -eq $PID) 'a live exact identity was lost'
$observed = Get-MendimaruExactStudioProcess -ExpectedProcessId $PID -ExpectedStartedTicks ($ticks - 1)
Assert ($null -eq $observed) 'a reused PID was treated as the previous session'
$observed = Get-MendimaruExactStudioProcess -ExpectedProcessId ([int]::MaxValue) -ExpectedStartedTicks $ticks
Assert ($null -eq $observed) 'an absent PID was not authoritatively detected'

Add-Type -TypeDefinition @'
using System;
public class ProbeProcessFixture {
    public bool HasExited = false;
    public DateTime StartTime = new DateTime(638908128000000000L, DateTimeKind.Utc);
    public bool ThrowRefresh = false;
    public void Refresh() { if (ThrowRefresh) throw new Exception("observation unavailable"); }
}
'@
$script:ProbeLookupFails = $false
$script:ProbeProcess = New-Object ProbeProcessFixture
function Get-Process {
    param($Id, $ErrorAction)
    if ($script:ProbeLookupFails) { throw 'access denied' }
    return $script:ProbeProcess
}
$script:ProbeProcess.HasExited = $true
Assert ($null -eq (Get-MendimaruExactStudioProcess -ExpectedProcessId 42 -ExpectedStartedTicks 638908128000000000)) 'observed exit was lost'
$script:ProbeProcess.HasExited = $false
foreach ($failure in @('lookup', 'refresh')) {
    $script:ProbeLookupFails = $failure -eq 'lookup'
    $script:ProbeProcess.ThrowRefresh = $failure -eq 'refresh'
    $threw = $false
    try { $null = Get-MendimaruExactStudioProcess -ExpectedProcessId 42 -ExpectedStartedTicks 638908128000000000 }
    catch { $threw = $true }
    Assert $threw 'an observation failure was converted into session absence'
}
'Production Studio exact-identity probe: PASS'
