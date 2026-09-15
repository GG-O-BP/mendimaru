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
function Send-Request([string]$Operation='capabilities',[int]$Budget=15000) {
    Remove-Item -LiteralPath ($controlPath+'.ui.report') -Force -ErrorAction SilentlyContinue
    $script:TestId = 'ui_' + [Guid]::NewGuid().ToString('N')
    $payload = [ordered]@{id=$script:TestId;expiresAt=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+$Budget;request=@{sessionId=$session;operation=$Operation;timeoutMs=$Budget}}
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

    # Invalid authentication never reaches the worker or consumes a sequence.
    Remove-Item -LiteralPath ($controlPath+'.ui.report') -Force
    [IO.File]::WriteAllText(($controlPath+'.ui.request'),'{"schemaVersion":1,"mac":"invalid"}')
    $sequence=$script:UiSequence
    Service-MendimaruUi
    Assert ($null -eq $script:UiWorker -and $script:UiSequence -eq $sequence -and -not (Test-Path ($controlPath+'.ui.report'))) 'unauthenticated request was accepted'
    Remove-Item -LiteralPath ($controlPath+'.ui.request') -Force
    Write-Output 'UI helper: persistent worker, wrong target, crash/restart, release, cancellation, expired request, UTF-8 and authentication passed.'
} finally {
    Stop-MendimaruUi
    $script:MendimaruHmac.Dispose()
    [Array]::Clear($key,0,$key.Length)
    Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue
}
