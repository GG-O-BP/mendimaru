# Production authenticated monitor protocol, without opening RDP or using UIA.
$ErrorActionPreference='Stop'
$source=Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\scripts'
$lab=Join-Path $env:TEMP ('mendimaru-query-test-'+[Guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $lab
try {
    $env:MENDIMARU_REQUEST_ID=[Guid]::NewGuid().ToString('N')
    $env:MENDIMARU_OPERATION_NONCE=[Guid]::NewGuid().ToString('N')
    $env:MENDIMARU_OPERATION_KEY=[Convert]::ToBase64String((New-Object byte[] 32))
    . (Join-Path $source 'operation_security.ps1')
    . (Join-Path $source 'session_query.ps1')
    $controlPath=Join-Path $lab 'control'
    $script:UiIdentity=@{sessionId='studio-4242-638908128000000000'}
    $script:Queries=0
    function Get-MendimaruGuestSessions { $script:Queries++; return @(@{sessionId=$script:UiIdentity.sessionId;processId=4242;version='11.12.3';startedAt='2025-08-15T00:00:00Z';hasWindow=$true}) }
    $id='query_'+[Guid]::NewGuid().ToString('N')
    $resultPath=$controlPath+'.sessions.request'
    Write-MendimaruReport @{id=$id;sessionId=$script:UiIdentity.sessionId;expiresAt=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+30000}
    $original=[IO.File]::ReadAllText($resultPath)
    Service-MendimaruSessionQuery
    $reply=(Read-MendimaruAuthenticatedPayload ($controlPath+'.sessions.report')).Json|ConvertFrom-Json
    if(-not $reply.ok -or $reply.id -cne $id -or @($reply.sessions).Count -ne 1 -or $script:Queries -ne 1){throw 'guest query protocol failed'}
    # Same signed sequence cannot be replayed even after the request disappears.
    [IO.File]::WriteAllText($resultPath,$original)
    Service-MendimaruSessionQuery
    if($script:Queries -ne 1){throw 'query replay accepted'}
    Remove-Item $resultPath
    foreach($invalid in @('expired','wrong-session','wrong-id')){
        Write-MendimaruReport @{id=$(if($invalid -ceq 'wrong-id'){'query_old'}else{'query_'+[Guid]::NewGuid().ToString('N')});sessionId=$(if($invalid -ceq 'wrong-session'){'studio-9999-638908128000000000'}else{$script:UiIdentity.sessionId});expiresAt=([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()+$(if($invalid -ceq 'expired'){-60000}else{30000}))}
        Service-MendimaruSessionQuery
        if($script:Queries -ne 1){throw 'invalid query accepted'}
        Remove-Item $resultPath
    }
    Write-Output 'Session discovery: authenticated, exact identity, replay and expiry checks passed.'
} finally {Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue}
