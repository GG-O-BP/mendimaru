# Called only by the explicit env repair-clock operation. RTC basis is supplied
# by the operator after inspecting QEMU's clock and the container timezone.
function Test-MendimaruClockAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Repair-MendimaruGuestClock {
    param([ValidateSet('utc', 'local')][string]$RtcBase)
    if ([string]::IsNullOrEmpty($RtcBase)) { throw 'guest-clock-rtc-base-required' }
    if (-not (Test-MendimaruClockAdministrator)) { throw 'guest-clock-administrator-required' }
    # Refuse to change time while Studio owns process/session timestamps.
    if (@(Get-CimInstance Win32_Process -Filter "Name='studiopro.exe'" -ErrorAction Stop).Count -gt 0) {
        throw 'guest-clock-studio-running'
    }
    $path = 'HKLM:\SYSTEM\CurrentControlSet\Control\TimeZoneInformation'
    $value = if ($RtcBase -eq 'utc') { 1 } else { 0 }
    $null = New-ItemProperty -Path $path -Name RealTimeIsUniversal -PropertyType DWord -Value $value -Force -ErrorAction Stop
    Set-Service -Name W32Time -StartupType Automatic -ErrorAction Stop
    Start-Service -Name W32Time -ErrorAction Stop
}
