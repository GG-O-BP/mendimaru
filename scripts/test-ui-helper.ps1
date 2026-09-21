# Windows contract tests with a non-Studio process. This is not native Studio
# acceptance: it verifies the production supervisor/worker and failure paths.
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'Windows required' }
$root = Split-Path $PSScriptRoot -Parent
$source = Join-Path $root 'src-tauri\scripts'
$lab = Join-Path $env:TEMP ('mendimaru-ui-test-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $lab
$controlPath = Join-Path $lab 'control.json'
$resultPath = Join-Path $lab 'launch.json'
$key = New-Object byte[] 32
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
$rng.GetBytes($key); $rng.Dispose()
$env:MENDIMARU_REQUEST_ID = [Guid]::NewGuid().ToString('N')
$env:MENDIMARU_OPERATION_NONCE = [Guid]::NewGuid().ToString('N')
$env:MENDIMARU_OPERATION_KEY = [Convert]::ToBase64String($key)
. (Join-Path $source 'operation_security.ps1')
$worker = [IO.File]::ReadAllBytes((Join-Path $source 'ui_worker.ps1'))
$supervisor = [IO.File]::ReadAllText((Join-Path $source 'ui_supervisor.ps1')).Replace('__UI_WORKER_BASE64__',[Convert]::ToBase64String($worker))
# Test-only capture of initialization stderr: this worker receives no secrets.
# Production drains and discards stderr instead of exposing guest exceptions.
$supervisor=$supervisor.Replace('$script:UiWorker.BeginErrorReadLine()', '$script:TestWorkerStderr=$script:UiWorker.StandardError.ReadToEndAsync()')
. ([ScriptBlock]::Create($supervisor))
$process = Get-Process -Id $PID
$session = 'studio-' + $PID + '-' + $process.StartTime.ToUniversalTime().Ticks
Initialize-MendimaruUi $process $session
function Assert([bool]$Condition,[string]$Message) { if(-not $Condition){throw $Message} }
function Send-Request([string]$Operation='capabilities',[int]$Budget=15000,[int]$ExpiresExtra=0) {
    Remove-Item -LiteralPath ($controlPath+'.ui.report') -Force -ErrorAction SilentlyContinue
    $script:TestId = 'ui_' + [Guid]::NewGuid().ToString('N')
    $payload = [ordered]@{id=$script:TestId;expiresAt=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+$Budget+$ExpiresExtra;request=@{sessionId=$session;operation=$Operation;timeoutMs=$Budget}}
    $resultPath = $controlPath+'.ui.request'
    Write-MendimaruReport $payload
    Service-MendimaruUi
}
function Receive-Response {
    $until=[DateTime]::UtcNow.AddSeconds(20)
    while(-not (Test-Path -LiteralPath ($controlPath+'.ui.report')) -and [DateTime]::UtcNow -lt $until){Service-MendimaruUi;Start-Sleep -Milliseconds 25}
    Assert (Test-Path -LiteralPath ($controlPath+'.ui.report')) 'supervisor response timeout'
    $reply=Read-MendimaruAuthenticatedPayload -Path ($controlPath+'.ui.report')
    $payload=$reply.Json|ConvertFrom-Json
    Assert ($payload.id -ceq $script:TestId) 'wrong response identity'
    return $payload
}
try {
    & {
        # Exercise the production inventory header with multiple installations.
        # A nested Object[] used to make .path.Equals throw and silently hide
        # every running Studio session from query/reconnect under PowerShell 5.1.
        $known=@(@{version='11.12.4';path='C:\Mendix\11.12.4\modeler\studiopro.exe'},@{version='10.24.26';path='C:\Mendix\10.24.26\modeler\studiopro.exe'})
        $json=ConvertTo-Json -InputObject $known -Compress
        $template=[IO.File]::ReadAllText((Join-Path $source 'studio_sessions.ps1'))
        $header=$template.Substring(0,$template.IndexOf('__SECURITY_PREAMBLE__')).Replace('__TARGET_PROCESS_ID__','0').Replace('__TARGET_STARTED_TICKS__','0').Replace('__KNOWN_STUDIOS_BASE64__',[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($json)))
        . ([ScriptBlock]::Create($header))
        $match=@($knownStudios|Where-Object{$_.path.Equals('c:\mendix\11.12.4\modeler\StudioPro.exe',[StringComparison]::OrdinalIgnoreCase)})
        Assert ($knownStudios.Count -eq 2 -and $match.Count -eq 1 -and $match[0].version -ceq '11.12.4') 'installed Studio inventory did not preserve separate paths'
    }
    # Exercise the production pipe writer with a Windows PowerShell child.
    # ASCII JSON succeeds even with the old code-page-dependent StreamWriter;
    # Korean and supplementary characters must survive byte-for-byte too.
    $echoPath=Join-Path $lab 'echo.ps1'
    [IO.File]::WriteAllText($echoPath, '[Console]::InputEncoding=New-Object Text.UTF8Encoding($false);[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false);[Console]::WriteLine([Console]::ReadLine())')
    $echoStart=New-Object Diagnostics.ProcessStartInfo
    $echoStart.FileName=Join-Path $PSHOME 'powershell.exe'
    $echoStart.Arguments='-NoLogo -NoProfile -NonInteractive -File "'+$echoPath+'"'
    $echoStart.UseShellExecute=$false;$echoStart.CreateNoWindow=$true
    $echoStart.RedirectStandardInput=$true;$echoStart.RedirectStandardOutput=$true
    $echoStart.StandardOutputEncoding=New-Object Text.UTF8Encoding($false)
    $echoProcess=New-Object Diagnostics.Process
    $echoProcess.StartInfo=$echoStart
    $testEncoding=[Console]::InputEncoding
    try {
        [Console]::InputEncoding=[Text.Encoding]::UTF8
        Start-MendimaruUiProcess $echoProcess
        $unicode=([string][char]0xd30c)+[char]0xc77c+[char]::ConvertFromUtf32(0x1f642)
        $line=ConvertTo-Json -InputObject @{name=$unicode} -Compress
        Send-MendimaruUiLine $echoProcess $line
        $read=$echoProcess.StandardOutput.ReadLineAsync()
        Assert ($read.Wait(15000)) 'UTF-8 pipe response timeout'
        Assert ($read.Result -ceq $line) 'UTF-8 request pipe corrupted Unicode'
    } finally {
        [Console]::InputEncoding=$testEncoding
        if(-not $echoProcess.HasExited){$echoProcess.Kill()}
        $null=$echoProcess.WaitForExit(2000);$echoProcess.Dispose()
    }
    Send-Request
    $first=Receive-Response
    if($first.reason -ceq 'ui-helper-exited' -and $script:TestWorkerStderr.IsCompleted){
        $diagnostic=$script:TestWorkerStderr.GetAwaiter().GetResult()
        Write-Output ('Worker initialization diagnostic: '+$diagnostic.Substring(0,[Math]::Min(8192,$diagnostic.Length)))
    }
    Assert (-not $first.ok -and $first.reason -ceq 'ui-wrong-session') ('worker must reject a non-Studio target: '+$first.reason)
    $workerId=$script:UiWorker.Id
    $workerBytes=[IO.File]::ReadAllBytes((Join-Path $script:UiDirectory 'worker.ps1'))
    Assert ($workerBytes[0] -eq 239 -and $workerBytes[1] -eq 187 -and $workerBytes[2] -eq 191) 'worker script must preserve UTF-8 selectors on Windows PowerShell 5.1'
    Send-Request
    $second=Receive-Response
    Assert ($second.reason -ceq 'ui-wrong-session' -and $script:UiWorker.Id -eq $workerId) 'warm command must reuse its worker'

    # Crash only this test's job-owned helper; a later request must get a new worker.
    $script:UiWorker.Kill();$script:UiWorker.WaitForExit()
    Send-Request
    $third=Receive-Response
    Assert ($third.reason -ceq 'ui-wrong-session' -and $script:UiWorker.Id -ne $workerId) 'crashed worker did not restart'

    # A signed cancellation is handled before a completed worker response.
    Send-Request
    $resultPath=$controlPath+'.ui.cancel'
    Write-MendimaruReport @{id=$script:TestId;cancel=$true}
    $resultPath=Join-Path $lab 'launch.json'
    Service-MendimaruUi
    $cancelled=Receive-Response
    Assert ($cancelled.reason -ceq 'ui-cancelled' -and $null -eq $script:UiWorker) 'cancellation did not stop its owned worker'

    Send-Request 'release'
    $released=Receive-Response
    Assert ($released.ok -and $null -eq $script:UiWorker -and $null -eq $script:UiDirectory) 'release left owned resources'

    # An already-expired signed request cannot start a helper.
    Send-Request 'capabilities' -1
    $expired=Receive-Response
    Assert ($expired.reason -ceq 'ui-request-expired' -and $null -eq $script:UiWorker) 'expired request started work'

    # A request stamped slightly beyond the acceptance cap stays acceptable:
    # the guest clock may trail the Linux host clock by a bounded grace
    # (docs/winboat-clock-sync.md). Beyond the grace the request is rejected.
    # The extra offset keeps the worker timeout itself inside its own cap.
    Send-Request 'capabilities' 15000 48000
    $graced=Receive-Response
    Assert ($graced.ok) 'request within clock grace was rejected'
    Send-Request 'release'
    $null=Receive-Response
    Assert ($null -eq $script:UiWorker) 'clock-grace request left a worker running'
    Send-Request 'capabilities' 15000 52000
    $far=Receive-Response
    Assert ($far.reason -ceq 'ui-request-expired' -and $null -eq $script:UiWorker) 'far-future request was accepted'

    # Invalid authentication never reaches the worker or consumes a sequence.
    Remove-Item -LiteralPath ($controlPath+'.ui.report') -Force
    [IO.File]::WriteAllText(($controlPath+'.ui.request'),'{"schemaVersion":1,"mac":"invalid"}')
    $sequence=$script:UiSequence
    Service-MendimaruUi
    Assert ($null -eq $script:UiWorker -and $script:UiSequence -eq $sequence -and -not (Test-Path ($controlPath+'.ui.report'))) 'unauthenticated request was accepted'
    Remove-Item -LiteralPath ($controlPath+'.ui.request') -Force
    # A discarded owner retires its old monitor without closing Studio. The
    # same authenticated channel rejects a different session and bad signatures.
    $closePath=$controlPath+'.ui.close'
    [IO.File]::WriteAllText($closePath,'{"schemaVersion":1,"mac":"invalid"}')
    Service-MendimaruUi
    Assert (-not $script:UiClosed) 'unauthenticated monitor close was accepted'
    Remove-Item -LiteralPath $closePath -Force
    $resultPath=$closePath
    Write-MendimaruReport @{sessionId='studio-other';close=$true}
    Service-MendimaruUi
    Assert (-not $script:UiClosed) 'another session retired this monitor'
    Remove-Item -LiteralPath $closePath -Force
    Write-MendimaruReport @{sessionId=$session;close=$true}
    Service-MendimaruUi
    Assert ($script:UiClosed -and $null -eq $script:UiWorker -and -not (Test-Path $closePath)) 'owned monitor did not retire'
    Write-Output 'UI helper: persistent worker, wrong target, crash/restart, release, cancellation, expired request, clock grace, UTF-8 and authentication passed.'
} finally {
    Stop-MendimaruUi
    $script:MendimaruHmac.Dispose()
    [Array]::Clear($key,0,$key.Length)
    Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue
}
