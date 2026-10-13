# The authenticated monitor owns one exact Studio PID/start time. A launch
# window can consume WM_CLOSE then be replaced by Sign In or the main window.
# Keep the owner's stop intent across that handoff, without force-killing the
# process or answering dialogs. Each distinct main window gets at most one
# accepted close per explicit stop request; a disabled/missing window is not
# delivery and can be observed again. Stopwatch time survives guest clock sync.
function New-MendimaruStudioStopState {
    return [pscustomobject]@{
        DeliveredHandles = New-Object 'System.Collections.Generic.HashSet[long]'
        LastAttempt = [long]0
        Deadline = [Diagnostics.Stopwatch]::GetTimestamp() + ([Diagnostics.Stopwatch]::Frequency * 30)
    }
}

function Invoke-MendimaruPendingStudioStop {
    param(
        $Process,
        $State,
        [int]$ExpectedProcessId,
        [long]$ExpectedStartedTicks
    )
    if ($null -eq $State) { return 'idle' }
    $now = [Diagnostics.Stopwatch]::GetTimestamp()
    if ($now -ge $State.Deadline) { return 'expired' }
    if ($State.LastAttempt -ne 0 -and
        ($now - $State.LastAttempt) -lt ([Diagnostics.Stopwatch]::Frequency / 4)) {
        return 'waiting'
    }
    $State.LastAttempt = $now
    try {
        $Process.Refresh()
        if ($Process.HasExited) { return 'ended' }
        if ([int]$Process.Id -ne $ExpectedProcessId -or
            $Process.StartTime.ToUniversalTime().Ticks -ne $ExpectedStartedTicks) {
            return 'identity-mismatch'
        }
        $handle = [long]$Process.MainWindowHandle.ToInt64()
        if ($handle -eq 0 -or $State.DeliveredHandles.Contains($handle)) {
            return 'waiting'
        }
        # False means absent or disabled, not "Studio was asked to close".
        # True is only message delivery: exit still needs a later observation.
        if ($Process.CloseMainWindow()) {
            $null = $State.DeliveredHandles.Add($handle)
            return 'delivered'
        }
    } catch {
        # An unavailable process/window observation never confirms absence and
        # never expands the target beyond the authenticated process identity.
    }
    return 'waiting'
}
