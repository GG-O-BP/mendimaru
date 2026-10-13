# Deterministic contract tests of the production guest stop state machine.
# No Windows process, real Studio installation, or user desktop is touched.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $root 'src-tauri/scripts/studio_stop.ps1')
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
public class StopProcessFixture {
    public int Id = 42;
    public bool HasExited = false;
    public DateTime StartTime = new DateTime(638900000000000000L, DateTimeKind.Utc);
    public IntPtr MainWindowHandle = new IntPtr(10);
    public bool Accept = true;
    public bool ThrowRefresh = false, ThrowClose = false;
    public List<long> CloseCalls = new List<long>();
    public int RefreshCalls = 0;
    public void Refresh() { RefreshCalls++; if(ThrowRefresh) throw new Exception("unavailable"); }
    public bool CloseMainWindow() {
        CloseCalls.Add(MainWindowHandle.ToInt64());
        if(ThrowClose) throw new Exception("unavailable");
        return Accept;
    }
}
'@
function Assert([bool]$Condition, [string]$Message) { if(-not $Condition){throw $Message} }
function Poll($Process, $State) {
    # Bypass just the time throttle so cases do not rely on sleeps or clocks.
    if ($State) { $State.LastAttempt = [long]0 }
    return Invoke-MendimaruPendingStudioStop -Process $Process -State $State `
        -ExpectedProcessId 42 -ExpectedStartedTicks 638900000000000000
}
$process = New-Object StopProcessFixture
Assert ((Poll $process $null) -eq 'idle') 'no stop intent must not close a window'
Assert ($process.CloseCalls.Count -eq 0) 'idle changed the process'
$state = New-MendimaruStudioStopState
$process.MainWindowHandle = [IntPtr]::Zero
Assert ((Poll $process $state) -eq 'waiting') 'no current window is not delivery'
Assert ($process.CloseCalls.Count -eq 0) 'a missing window was closed'
$process.MainWindowHandle = [IntPtr]10
$process.Accept = $false
Assert ((Poll $process $state) -eq 'waiting') 'disabled window is not delivery'
Assert (-not $state.DeliveredHandles.Contains([long]10)) 'false close was remembered as delivered'
$process.Accept = $true
Assert ((Poll $process $state) -eq 'delivered') 'a later enabled window must receive the original intent'
Assert (-not $process.HasExited) 'message delivery must not be called process exit'
$before = $process.CloseCalls.Count
Assert ((Poll $process $state) -eq 'waiting') 'accepted close cannot be resent to the same root'
Assert ($process.CloseCalls.Count -eq $before) 'duplicate close could re-open a cancelled save dialog'
# Startup/sign-in handoff: an accepted close of the old root is not enough.
$process.MainWindowHandle = [IntPtr]20
Assert ((Poll $process $state) -eq 'delivered') 'new main window lost the authenticated stop intent'
$process.MainWindowHandle = [IntPtr]10
Assert ((Poll $process $state) -eq 'waiting') 'old root must not receive another implicit close'
# Only another explicit authenticated owner request resets delivered windows.
$state = New-MendimaruStudioStopState
Assert ((Poll $process $state) -eq 'delivered') 'explicit owner retry must work'
$before = $process.CloseCalls.Count
$process.HasExited = $true
Assert ((Poll $process $state) -eq 'ended') 'exit must be observed, not inferred from WM_CLOSE'
Assert ($process.CloseCalls.Count -eq $before) 'an exited process was acted on'
foreach ($mismatch in @('pid','start')) {
    $process = New-Object StopProcessFixture
    if ($mismatch -eq 'pid') { $process.Id = 99 } else { $process.StartTime = $process.StartTime.AddTicks(1) }
    Assert ((Poll $process (New-MendimaruStudioStopState)) -eq 'identity-mismatch') 'identity mismatch not rejected'
    Assert ($process.CloseCalls.Count -eq 0) 'PID/start-time mismatch closed an unrelated process'
}
$process = New-Object StopProcessFixture
$state = New-MendimaruStudioStopState
$process.ThrowRefresh = $true
Assert ((Poll $process $state) -eq 'waiting') 'unavailable observation is not absence'
Assert ($process.CloseCalls.Count -eq 0) 'failed observation acted on process'
$process.ThrowRefresh = $false
$process.ThrowClose = $true
Assert ((Poll $process $state) -eq 'waiting') 'throwing close is not delivery'
Assert ($state.DeliveredHandles.Count -eq 0) 'failed close was remembered'
$process.ThrowClose = $false
Assert ((Poll $process $state) -eq 'delivered') 'temporary close failure lost stop intent'
$state = New-MendimaruStudioStopState
$state.LastAttempt = [Diagnostics.Stopwatch]::GetTimestamp()
$before = $process.RefreshCalls
$status = Invoke-MendimaruPendingStudioStop -Process $process -State $state `
    -ExpectedProcessId 42 -ExpectedStartedTicks 638900000000000000
Assert ($status -eq 'waiting' -and $process.RefreshCalls -eq $before) 'pending polling is not throttled'
$state.Deadline = [long]0
Assert ((Poll $process $state) -eq 'expired') 'stop intent must not affect future windows after its deadline'
Assert ($process.RefreshCalls -eq $before) 'expired stop still acted on the process'
'Production Studio stop helper: PASS'
