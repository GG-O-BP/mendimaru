# A missing PID or a different start time proves only this exact identity ended.
# Access/observation failures must never be converted to an empty session list.
function Get-MendimaruExactStudioProcess {
    param([int]$ExpectedProcessId, [long]$ExpectedStartedTicks)

    try {
        $process = Get-Process -Id $ExpectedProcessId -ErrorAction Stop
    } catch {
        if ($_.FullyQualifiedErrorId -eq 'NoProcessFoundForGivenId,Microsoft.PowerShell.Commands.GetProcessCommand') {
            return $null
        }
        throw
    }
    $process.Refresh()
    if ($process.HasExited -or
        $process.StartTime.ToUniversalTime().Ticks -ne $ExpectedStartedTicks) {
        return $null
    }
    return $process
}
