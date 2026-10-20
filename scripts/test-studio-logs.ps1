# Production parser/redactor against a bounded fixture. No Studio/model/DB access.
$ErrorActionPreference='Stop'
$source=Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\scripts'
Add-Type -TypeDefinition ([IO.File]::ReadAllText((Join-Path $source 'studio_logs.cs')))
. (Join-Path $source 'studio_logs.ps1')
$lab=Join-Path $env:TEMP ('mendimaru-studio-logs-'+[Guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $lab
$script:MendimaruHmac=New-Object Security.Cryptography.HMACSHA256
$script:MendimaruHmac.Key=New-Object byte[] 32
try {
    $path=Join-Path $lab 'log.txt'
    $canary='STUDIO_SECRET_CANARY_268'
    $fixture=@"
2026-10-20 12:00:00.1234 ERROR Mendix.Modeler.Run An error occurred while trying to run your app.
2026-10-20 12:00:00.1234 ERROR Mendix.Modeler.Run <0:00 $canary>: System.InvalidOperationException (0x80131509): password=$canary
 ---> System.Net.Http.HttpRequestException: Authorization: Bearer $canary
 ---> System.Net.Sockets.SocketException (10061): User ID=admin;Password=$canary;Server=private
   at Mendix.Modeler.Run.Start(String password) in C:\Users\$canary\source.cs:line 12
   --- End of inner exception stack trace ---
2026-10-20 12:00:01.1234 ERROR Mendix.Modeler.Run token: $canary
$canary
Cookie: session=$canary
https://admin:$canary@private.example/?token=$canary
2026-10-20 12:00:02.1234 ERROR Mendix.Modeler.Run no error
"@
    [IO.File]::WriteAllText($path,$fixture+"`r`n",(New-Object Text.UTF8Encoding($false)))
    $before=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
    $link=Join-Path $lab 'linked.txt'
    $null=New-Item -ItemType HardLink -Path $link -Target $path
    $rejected=$false
    try {$unexpected=[MendimaruStudioLogFile]::Open($path);$unexpected.Dispose()}catch{$rejected=$true}
    if(-not $rejected){throw 'multiply linked diagnostic accepted'}
    Remove-Item -LiteralPath $link -Force
    $stream=[MendimaruStudioLogFile]::Open($path)
    $options=[pscustomobject]@{maxBytes=1024;maxLines=2;since=$null;until=$null;cursor=$null}
    $session='studio-4242-639280000000000000'
    $started=[DateTime]::Parse('2026-10-19T00:00:00Z').ToUniversalTime().Ticks
    $all=New-Object Collections.Generic.List[string];$pages=0
    do {
        $page=Get-MendimaruStudioLogPage $options $stream $session $started
        $json=$page|ConvertTo-Json -Depth 10 -Compress
        if($json.Contains($canary)){throw 'canary exposed'}
        if($page.bytes -gt 1024 -or $page.lines.Count -gt 2){throw 'bounds exceeded'}
        foreach($line in $page.lines){$all.Add($line)}
        $options.cursor=$page.nextCursor;$pages++
        if($pages -gt 20){throw 'cursor did not advance'}
    }while($page.truncated)
    $combined=$all -join "`n"
    if($combined -notmatch 'InvalidOperationException \(0x80131509\)' -or $combined -notmatch 'HttpRequestException' -or $combined -notmatch 'SocketException \(10061\)' -or $combined -notmatch 'at Mendix.Modeler.Run.Start\(\.\.\.\)'){throw 'exception chain/stack lost'}
    foreach($bad in @('foreign-session','tampered','time-filter')){
        $copy=[pscustomobject]@{maxBytes=1024;maxLines=2;since=$null;until=$null;cursor=$options.cursor}
        $target=$session
        if($bad -eq 'foreign-session'){$target='studio-9999-639280000000000001'}
        if($bad -eq 'tampered'){$copy.cursor=$copy.cursor.Substring(0,$copy.cursor.Length-1)+'x'}
        if($bad -eq 'time-filter'){$copy.since='2026-10-20T00:00:00Z'}
        $rejected=$false
        try {$null=Get-MendimaruStudioLogPage $copy $stream $target $started}catch{$rejected=$_.Exception.Message -match 'studio-logs-cursor-invalid'}
        if(-not $rejected){throw 'foreign/tampered cursor accepted'}
    }
    $options.cursor=$null;$options.since='2026-10-21T00:00:00Z';$options.maxLines=200
    $empty=Get-MendimaruStudioLogPage $options $stream $session $started
    if($empty.lines.Count -ne 0){throw 'time filter ignored'}
    if((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -cne $before){throw 'collection modified log'}
    # An opened file can belong to one exact process, another process, or
    # concurrent writers. The production ownership primitive distinguishes all.
    $writerSource='$f=[IO.File]::Open($env:MENDIMARU_LOG_FIXTURE,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::ReadWrite);try{[Console]::WriteLine("ready");$null=[Console]::ReadLine()}finally{$f.Dispose()}'
    $env:MENDIMARU_LOG_FIXTURE=$path
    $writers=New-Object Collections.Generic.List[Diagnostics.Process]
    try {
        foreach($index in @(0,1)){
            $start=New-Object Diagnostics.ProcessStartInfo
            $start.FileName=(Join-Path $PSHOME 'powershell.exe')
            $start.Arguments='-NoLogo -NoProfile -NonInteractive -EncodedCommand '+[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($writerSource))
            $start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.RedirectStandardInput=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
            $writer=[Diagnostics.Process]::Start($start);$writers.Add($writer)
            $ready=$writer.StandardOutput.ReadLineAsync()
            if(-not $ready.Wait(10000) -or $ready.Result -cne 'ready'){throw 'writer fixture failed'}
            if($index -eq 0){
                if(-not [MendimaruStudioLogFile]::Owned($path,$writer.Id,$writer.StartTime.ToUniversalTime().Ticks)){throw 'exact writer ownership not found'}
                if([MendimaruStudioLogFile]::Owned($path,$writer.Id,$writer.StartTime.ToUniversalTime().Ticks+1)){throw 'reused PID identity accepted'}
            }else{
                if([MendimaruStudioLogFile]::Owned($path,$writers[0].Id,$writers[0].StartTime.ToUniversalTime().Ticks)){throw 'shared log accepted as isolated'}
            }
        }
    } finally {
        foreach($writer in $writers){try{$writer.StandardInput.WriteLine();if(-not $writer.WaitForExit(3000)){$writer.Kill()}}finally{$writer.Dispose()}}
        Remove-Item Env:MENDIMARU_LOG_FIXTURE -ErrorAction SilentlyContinue
    }
    # A cursor pins content as well as identity/length; in-place overwrites
    # must not silently serve a different record from the original snapshot.
    $options.cursor=$null;$options.since=$null;$options.maxLines=1
    $original=Get-MendimaruStudioLogPage $options $stream $session $started
    $mutator=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::ReadWrite)
    try{$mutator.WriteByte(88);$mutator.Flush()}finally{$mutator.Dispose()}
    $options.cursor=$original.nextCursor;$rejected=$false
    try{$null=Get-MendimaruStudioLogPage $options $stream $session $started}catch{$rejected=$_.Exception.Message -match 'studio-logs-cursor-invalid'}
    if(-not $rejected){throw 'overwritten snapshot accepted'}

    $stream.Dispose();$stream=$null
    Write-Output 'Studio diagnostics: secret canary, nested causes, stack symbols, byte/line bounds, cursor isolation/tamper, timezone filtering, and read-only checks passed.'
} finally {
    if($null -ne $stream){$stream.Dispose()};$script:MendimaruHmac.Dispose()
    Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue
}
