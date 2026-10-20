# Embedded in the authenticated keeper. No caller-supplied paths or executable code.
function Protect-MendimaruStudioLogLine([string]$Line) {
    # Fail closed: arbitrary messages, argument values, URLs, connection strings,
    # headers, paths and multiline credentials never leave the guest. Preserve
    # structural diagnostics (exception types, inner-chain markers, stack symbols).
    $prefix=''
    if($Line -match '^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d{1,7})\s+(TRACE|DEBUG|INFO|WARN|ERROR|FATAL)\s+([A-Za-z_][A-Za-z0-9_.`+<>]*)\s+(.*)$'){
        $prefix=$Matches[1]+' '+$Matches[2]+' '
        $Line=$Matches[4]
    }
    # NLog activity scopes are context, never diagnostic content to export.
    if($prefix){$Line=$Line -replace '^<\d+:\d{2} [^>\r\n]{1,128}>:\s*',''}
    if($Line -match '^\s*(?:--->\s*(?:\(Inner Exception #[0-9]+\)\s*)?)?((?:[A-Za-z_][A-Za-z0-9_`+]*\.)+(?:[A-Za-z0-9_`+]*Exception|[A-Za-z0-9_`+]*Error))(?:\s*\((-?\d{1,10}|0x[0-9a-f]{8})\))?(?::|\s|$)'){
        $type=$Matches[1];$code=$Matches[2]
        $chain=$(if($Line -match '^\s*--->'){' ---> '}else{''})
        return $prefix+$chain+$type+$(if($code){' ('+$code+')'}else{''})+': [message omitted]'
    }
    if($Line -match '^\s*--- End of inner exception stack trace ---\s*$'){return $prefix+'   --- End of inner exception stack trace ---'}
    if($Line -match '^\s*--- End of stack trace from previous location ---\s*$'){return $prefix+'   --- End of stack trace from previous location ---'}
    if($Line -match '^\s*at ((?:[A-Za-z_][A-Za-z0-9_`+<>]*\.)+[A-Za-z_][A-Za-z0-9_`+<>]*(?:\([^"''\r\n]*\)))'){
        # Parameter lists can contain user values in unusual providers. Export
        # the method symbol alone; source file paths and parameter text are omitted.
        $symbol=$Matches[1] -replace '\(.*$',''
        return $prefix+'   at '+$symbol+'(...)'
    }
    foreach($safe in @('An error occurred while trying to run your app.','An error occurred while sending the request.','The response ended prematurely. (ResponseEnded)','The system cannot find the file specified','Access is denied')){
        if($Line -ceq $safe){return $prefix+$safe}
    }
    return $prefix+'[message omitted]'
}
function Get-MendimaruStudioLogMac([string]$Value) {
    return ([BitConverter]::ToString($script:MendimaruHmac.ComputeHash([Text.Encoding]::UTF8.GetBytes('studio-logs-v1:'+ $Value)))).Replace('-','').ToLowerInvariant()
}
function Get-MendimaruStudioLogPage($Options,[IO.FileStream]$Stream,[string]$SessionId,[long]$StartedTicks) {
    $maxBytes=[int]$Options.maxBytes;$maxLines=[int]$Options.maxLines
    if($maxBytes -lt 256 -or $maxBytes -gt 262144 -or $maxLines -lt 1 -or $maxLines -gt 2000){throw 'studio-logs-cursor-invalid'}
    $since=[DateTimeOffset]::new([DateTime]::new($StartedTicks,[DateTimeKind]::Utc))
    $until=[DateTimeOffset]::MaxValue
    if($Options.since){$s=[DateTimeOffset]::Parse([string]$Options.since,[Globalization.CultureInfo]::InvariantCulture);if($s -gt $since){$since=$s}}
    if($Options.until){$until=[DateTimeOffset]::Parse([string]$Options.until,[Globalization.CultureInfo]::InvariantCulture)}
    $identity=[MendimaruStudioLogFile]::Identity($Stream)
    $binding=$SessionId+'|'+$identity+'|'+[string]$Options.since+'|'+[string]$Options.until
    $position=[long]0;$end=$Stream.Length;$timestamp=$null
    if($end -gt 16777216){throw 'studio-logs-unreadable'}
    if($Options.cursor){
        $parts=([string]$Options.cursor).Split('.')
        if($parts.Count -ne 2 -or $parts[0] -cnotmatch '^[A-Za-z0-9_-]{1,900}$' -or $parts[1] -cnotmatch '^[0-9a-f]{64}$' -or (Get-MendimaruStudioLogMac ($binding+'|'+$parts[0])) -cne $parts[1]){throw 'studio-logs-cursor-invalid'}
        try {
            $encoded=$parts[0].Replace('-','+').Replace('_','/');$encoded=$encoded.PadRight($encoded.Length+(4-$encoded.Length%4)%4,'=')
            $state=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($encoded))|ConvertFrom-Json
            $position=[long]$state.position;$end=[long]$state.end;$timestamp=$state.timestamp
            if($position -lt 0 -or $end -lt $position -or $end -gt $Stream.Length){throw 'invalid'}
        }catch{throw 'studio-logs-cursor-invalid'}
    }
    # Do not export an unkeyed digest of secret-bearing source content.
    $fingerprint=Get-MendimaruStudioLogMac ('snapshot|'+[MendimaruStudioLogFile]::Fingerprint($Stream,$end))
    if($Options.cursor -and $state.fingerprint -cne $fingerprint){throw 'studio-logs-cursor-invalid'}
    $initialTimestamp=$timestamp
    $statePosition=$position
    $Stream.Position=$position;$scanEnd=[Math]::Min($end,$position+4194304)
    $lines=New-Object Collections.Generic.List[string];$bytes=0;$lineTruncated=$false;$incomplete=$false
    while($Stream.Position -lt $scanEnd){
        $before=$Stream.Position;$previousTimestamp=$timestamp;$oversized=$false;$partial=$false
        $line=[MendimaruStudioLogFile]::Line($Stream,$scanEnd,[ref]$oversized,[ref]$partial)
        if($partial){if($oversized -or $scanEnd -lt $end){throw 'studio-logs-unreadable'};$Stream.Position=$before;$incomplete=$true;break}
        if($line -match '^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d{1,7})\s'){$timestamp=$Matches[1]}
        if(-not $timestamp){continue}
        $time=[DateTime]::ParseExact($timestamp,@('yyyy-MM-dd HH:mm:ss.FFFFFFF'),[Globalization.CultureInfo]::InvariantCulture,[Globalization.DateTimeStyles]::None)
        # NLog timestamps are guest-local. Preserve the original and declare
        # the guest timezone; ambiguous DST timestamps cannot satisfy a filter.
        if([TimeZoneInfo]::Local.IsInvalidTime($time) -or [TimeZoneInfo]::Local.IsAmbiguousTime($time)){throw 'studio-logs-unreadable'}
        $observed=[DateTimeOffset]::new($time,[TimeZoneInfo]::Local.GetUtcOffset($time))
        if($observed -lt $since -or $observed -gt $until){continue}
        $safe=Protect-MendimaruStudioLogLine $line
        $length=[Text.Encoding]::UTF8.GetByteCount($safe)+1
        if($lines.Count -ge $maxLines -or $bytes+$length -gt $maxBytes){
            if($lines.Count -eq 0){$safe='[REDACTED oversized diagnostic]';$length=[Text.Encoding]::UTF8.GetByteCount($safe)+1;$lineTruncated=$true}
            else{$Stream.Position=$before;$timestamp=$previousTimestamp;break}
        }
        $lines.Add($safe);$bytes+=$length;$lineTruncated=$lineTruncated -or $oversized
    }
    $position=$Stream.Position
    $state=@{position=$position;end=$end;timestamp=$timestamp;fingerprint=$fingerprint}|ConvertTo-Json -Compress
    $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($state)).TrimEnd('=').Replace('+','-').Replace('/','_')
    $cursor=$encoded+'.'+(Get-MendimaruStudioLogMac ($binding+'|'+$encoded))
    # At EOF keep a cursor bound to the current snapshot. Call without a cursor
    # for a fresh snapshot; continuation never silently jumps to newer data.
    return @{sessionId=$SessionId;version='11.12.6';source='studio-internal';exceptionCount=@($lines|Where-Object {$_ -match '[.][A-Za-z0-9_`+]*(?:Exception|Error)(?: \((?:-?[0-9]+|0x[0-9a-f]{8})\))?: \[message omitted\]$'}).Count;initialTimestamp=$initialTimestamp;lastTimestamp=$timestamp;redactionPolicy='structural-only';guestTimeZone=[TimeZoneInfo]::Local.Id;lines=$lines.ToArray();bytes=$bytes;maxBytes=$maxBytes;maxLines=$maxLines;nextCursor=$cursor;truncated=(($position -lt $end -and -not $incomplete) -or $lineTruncated);incompleteLine=$incomplete;snapshotBytes=$end;scannedBytes=($position-$statePosition)}
}
function Get-MendimaruStudioLogs($Options) {
    $identity=$script:UiIdentity
    $process=Get-Process -Id ([int]$identity.processId) -ErrorAction SilentlyContinue
    if($null -eq $process -or $process.StartTime.ToUniversalTime().Ticks -ne [long]$identity.startedTicks -or -not [Mendimaru.ProcessSecurity]::IsCurrentUser($process.Id) -or $process.Path -cne $identity.executable){throw 'studio-logs-wrong-session'}
    if(([version]$process.MainModule.FileVersionInfo.FileVersion).ToString(4) -cne '11.12.6.0'){throw 'studio-logs-unsupported-version'}
    $null=Assert-MendimaruTrustedExecutable -Path $process.Path -Root $installRoot
    if(-not ('MendimaruStudioLogFile' -as [type])){Add-Type -TypeDefinition ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__STUDIO_LOGS_SOURCE_BASE64__')))}
    $root=Join-Path $env:LOCALAPPDATA 'Mendix\log\11.12.6'
    if(-not [IO.Directory]::Exists($root)){throw 'studio-logs-not-found'}
    $null=Assert-MendimaruDirectPath -Path $root -Root ([IO.Path]::GetPathRoot($root))
    $files=@(Get-ChildItem -LiteralPath $root -File -ErrorAction Stop|Where-Object {$_.Name -cmatch '^log(?:\.[0-9]{1,2})?\.txt$'})
    if($files.Count -eq 0){throw 'studio-logs-not-found'}
    if($files.Count -gt 32){throw 'studio-logs-unreadable'}
    $selected=$null;$selectedPath=$null
    try {
        foreach($file in $files){
            $stream=$null
            try {
                $null=Assert-MendimaruDirectPath -Path $file.FullName -Root $root -Leaf
                $stream=[MendimaruStudioLogFile]::Open($file.FullName)
                if([MendimaruStudioLogFile]::Owned($file.FullName,$process.Id,[long]$identity.startedTicks)){
                    if($null -ne $selected){throw 'studio-logs-unreadable'}
                    $selected=$stream;$selectedPath=$file.FullName;$stream=$null
                }
            }finally{if($null -ne $stream){$stream.Dispose()}}
        }
        if($null -eq $selected){throw 'studio-logs-wrong-session'}
        $page=Get-MendimaruStudioLogPage $Options $selected $identity.sessionId ([long]$identity.startedTicks)
        if(-not [MendimaruStudioLogFile]::Owned($selectedPath,$process.Id,[long]$identity.startedTicks)){throw 'studio-logs-wrong-session'}
        if($process.HasExited -or $process.StartTime.ToUniversalTime().Ticks -ne [long]$identity.startedTicks){throw 'studio-logs-wrong-session'}
        return $page
    }finally{if($null -ne $selected){$selected.Dispose()}}
}
