$ErrorActionPreference = 'Stop'
. (Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri/scripts/guest_clock.ps1')
function Assert([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
$script:Admin = $true
$script:Studio = $false
$script:LookupFails = $false
$script:Calls = @()
function Test-MendimaruClockAdministrator { return $script:Admin }
function Get-CimInstance {
    param($ClassName, $Filter, $ErrorAction)
    Assert ($ClassName -eq 'Win32_Process' -and $Filter -eq "Name='studiopro.exe'") 'wrong process guard'
    if ($script:LookupFails) { throw 'lookup failed' }
    if ($script:Studio) { [pscustomobject]@{Name='studiopro.exe'} }
}
function New-ItemProperty {
    param($Path, $Name, $PropertyType, $Value, [switch]$Force, $ErrorAction)
    Assert ($Path -eq 'HKLM:\SYSTEM\CurrentControlSet\Control\TimeZoneInformation') 'wrong registry path'
    Assert ($Name -eq 'RealTimeIsUniversal' -and $PropertyType -eq 'DWord') 'unexpected registry change'
    $script:Calls += "rtc:$Value"
}
function Set-Service {
    param($Name, $StartupType, $ErrorAction)
    Assert ($Name -eq 'W32Time' -and $StartupType -eq 'Automatic') 'wrong service configuration'
    $script:Calls += 'automatic'
}
function Start-Service { param($Name, $ErrorAction); $script:Calls += "start:$Name" }
foreach ($basis in @('utc', 'local')) {
    $script:Calls = @()
    Repair-MendimaruGuestClock -RtcBase $basis
    $value = if ($basis -eq 'utc') { 1 } else { 0 }
    Assert (($script:Calls -join ',') -eq "rtc:$value,automatic,start:W32Time") 'wrong RTC basis or synchronization order'
}
foreach ($failure in @('administrator', 'studio', 'lookup', 'missing-basis', 'invalid-basis')) {
    $script:Calls = @()
    $script:Admin = $failure -ne 'administrator'
    $script:Studio = $failure -eq 'studio'
    $script:LookupFails = $failure -eq 'lookup'
    $threw = $false
    try {
        if ($failure -eq 'missing-basis') { Repair-MendimaruGuestClock }
        elseif ($failure -eq 'invalid-basis') { Repair-MendimaruGuestClock -RtcBase 'guess' }
        else { Repair-MendimaruGuestClock -RtcBase utc }
    } catch { $threw = $true }
    Assert $threw "accepted $failure"
    Assert ($script:Calls.Count -eq 0) "changed guest before rejecting $failure"
}
'Production guest clock recovery: PASS'
