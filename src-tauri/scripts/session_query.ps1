# Authenticated, read-only discovery. No UIA, focus change, or RDP connection.
$script:SessionQuerySequence=[long]0
function Get-MendimaruGuestSessions {
    $errors=@()
    $processes=@(Get-Process -Name studiopro -ErrorAction SilentlyContinue -ErrorVariable errors)
    foreach($error in $errors){
        if($error.FullyQualifiedErrorId -notlike 'NoProcessFoundForGivenName,*'){throw 'session-query-failed'}
    }
    $sessions=New-Object Collections.Generic.List[object]
    foreach($process in $processes){
        if($sessions.Count -ge 64){throw 'session-query-failed'}
        if(-not [Mendimaru.ProcessSecurity]::IsCurrentUser([int]$process.Id)){continue}
        $process.Refresh()
        if($process.HasExited){continue}
        $path=[string]$process.Path
        if([string]::IsNullOrWhiteSpace($path)){throw 'session-query-failed'}
        # Only this installation's verified Studio executables are observable.
        $root=[IO.Path]::GetFullPath($installRoot).TrimEnd('\')+'\'
        if(-not $path.StartsWith($root,[StringComparison]::OrdinalIgnoreCase)){continue}
        $null=Assert-MendimaruTrustedExecutable -Path $path -Root $installRoot
        $started=$process.StartTime.ToUniversalTime()
        $version=[version]$process.MainModule.FileVersionInfo.FileVersion
        $sessions.Add(@{sessionId=('studio-{0}-{1}' -f $process.Id,$started.Ticks);version=$version.ToString(3);processId=[int]$process.Id;startedAt=$started.ToString('o');projectName=$null;hasWindow=($process.MainWindowHandle -ne [IntPtr]::Zero)})
    }
    return $sessions.ToArray()
}
function Service-MendimaruSessionQuery {
    if($null -eq $script:UiIdentity){return}
    $path=$controlPath+'.sessions.request'
    if(-not (Test-Path -LiteralPath $path)){return}
    try {
        $auth=Read-MendimaruAuthenticatedPayload -Path $path
        $packet=$auth.Json|ConvertFrom-Json
        if($auth.Sequence -le $script:SessionQuerySequence -or $packet.sessionId -cne $script:UiIdentity.sessionId -or $packet.id -cnotmatch '^query_[0-9a-f]{32}$'){return}
        $now=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        if([long]$packet.expiresAt -lt $now-5000 -or [long]$packet.expiresAt -gt $now+35000){return}
        $script:SessionQuerySequence=$auth.Sequence
        Remove-Item -LiteralPath $path -Force
        $response=@{id=$packet.id;sessionId=$packet.sessionId;ok=$false}
        try {$response.sessions=@(Get-MendimaruGuestSessions);$response.ok=$true}catch{}
        $resultPath=$controlPath+'.sessions.report'
        Write-MendimaruReport $response
    }catch{}
}
