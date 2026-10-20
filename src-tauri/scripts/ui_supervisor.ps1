# Included in the authenticated Studio launch/reconnect host. Worker source is
# embedded in the hash-verified launch script; requests can never replace it.
# Request deadlines are stamped from the Linux host clock; the guest clock may
# legitimately differ by a few seconds (docs/winboat-clock-sync.md), so the
# request window keeps a bounded grace on both comparisons.
$script:UiRequestClockGraceMs = [long]5000
$script:UiWorker = $null
$script:UiJob = $null
$script:UiTask = $null
$script:UiPending = $null
$script:UiDirectory = $null
$script:UiIdentity = $null
$script:UiSequence = [long]0
$script:UiVerified = $false
$script:UiClosed = $false

__HOST_SERVICE_SUPERVISOR__
__SESSION_QUERY__
__STUDIO_LOGS__

function Start-MendimaruUiProcess($Process) {
    # .NET Framework constructs StandardInput with Console.InputEncoding and
    # AutoFlush during Start(), which can emit a BOM before our UTF-8 bytes.
    # Scope a BOM-less encoding to construction, then restore the host setting.
    $previousEncoding=[Console]::InputEncoding
    try {
        [Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
        $null=$Process.Start()
    } finally { [Console]::InputEncoding=$previousEncoding }
}
function Send-MendimaruUiLine($Process,[string]$Line) {
    # Process.StandardInput uses the parent's console code page on .NET
    # Framework. Write UTF-8 bytes directly to match the worker's input stream.
    $bytes=[Text.Encoding]::UTF8.GetBytes($Line+"`n")
    $Process.StandardInput.BaseStream.Write($bytes,0,$bytes.Length)
    $Process.StandardInput.BaseStream.Flush()
}
function Initialize-MendimaruUi($Process, [string]$SessionId) {
    $script:UiIdentity = @{hostPlatform='linux';interactiveSessionId=[int]$Process.SessionId;sessionId=$SessionId;processId=[int]$Process.Id;startedTicks=$Process.StartTime.ToUniversalTime().Ticks.ToString();executable=$Process.Path;fileVersion=$Process.MainModule.FileVersionInfo.FileVersion}
}
function Stop-MendimaruUi {
    if($null -ne $script:UiJob){$script:UiJob.Dispose();$script:UiJob=$null}
    if($null -ne $script:UiWorker){
        try {if(-not $script:UiWorker.HasExited){$script:UiWorker.Kill()};$null=$script:UiWorker.WaitForExit(2000)}catch{}
        $script:UiWorker.Dispose();$script:UiWorker=$null
    }
    $script:UiTask=$null;$script:UiVerified=$false
    if($script:UiDirectory){Remove-Item -LiteralPath $script:UiDirectory -Recurse -Force -ErrorAction SilentlyContinue;$script:UiDirectory=$null}
}
function Close-MendimaruUi {
    Stop-MendimaruUi
    if($null -ne $script:UiPending){Write-MendimaruUiResult @{ok=$false;reason='ui-session-unavailable'}}
}
function Start-MendimaruUi {
    if($null -ne $script:UiWorker -and -not $script:UiWorker.HasExited){return}
    Stop-MendimaruUi
    if(-not ('MendimaruUiJob' -as [type])){
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public sealed class MendimaruUiJob : IDisposable {
    IntPtr handle;
    [StructLayout(LayoutKind.Sequential)] struct Basic {
        public long ProcessTime, JobTime; public uint Flags; public UIntPtr Min,Max; public uint Active; public UIntPtr Affinity; public uint Priority,Scheduling;
    }
    [StructLayout(LayoutKind.Sequential)] struct Io { public ulong A,B,C,D,E,F; }
    [StructLayout(LayoutKind.Sequential)] struct Extended { public Basic Basic; public Io Io; public UIntPtr ProcessMemory,JobMemory,PeakProcess,PeakJob; }
    [DllImport("kernel32.dll")] static extern IntPtr CreateJobObject(IntPtr a,IntPtr n);
    [DllImport("kernel32.dll")] static extern bool SetInformationJobObject(IntPtr h,int c,ref Extended info,int n);
    [DllImport("kernel32.dll")] static extern bool AssignProcessToJobObject(IntPtr j,IntPtr p);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    public MendimaruUiJob(IntPtr process) {
        handle=CreateJobObject(IntPtr.Zero,IntPtr.Zero);
        var e=new Extended(); e.Basic.Flags=0x2000|0x100; e.ProcessMemory=(UIntPtr)(512UL*1024*1024);
        if(handle==IntPtr.Zero || !SetInformationJobObject(handle,9,ref e,Marshal.SizeOf(e)) || !AssignProcessToJobObject(handle,process)) { Dispose();throw new InvalidOperationException("ui-provider-failed"); }
    }
    [DllImport("wtsapi32.dll")] static extern bool WTSQuerySessionInformation(IntPtr s,int id,int c,out IntPtr p,out int n);
    [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr p);
    public static bool Active(int id) {
        IntPtr p; int n;
        if (!WTSQuerySessionInformation(IntPtr.Zero,id,8,out p,out n)) return false;
        try { return n >= 4 && Marshal.ReadInt32(p) == 0; } finally { WTSFreeMemory(p); }
    }
    public void Dispose(){if(handle!=IntPtr.Zero){CloseHandle(handle);handle=IntPtr.Zero;}}
}
'@
    }
    $temp=[IO.Path]::GetFullPath($env:TEMP)
    if($temp -notmatch '^[A-Za-z]:\\'){throw 'ui-provider-failed'}
    $null=Assert-MendimaruDirectPath -Path $temp -Root ([IO.Path]::GetPathRoot($temp))
    $directory=Join-Path $temp ('mendimaru-ui-'+[Guid]::NewGuid().ToString('N'))
    $acl=New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true,$false)
    $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl.SetOwner($sid)
    $rule=New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')
    $acl.AddAccessRule($rule)
    $null=[IO.Directory]::CreateDirectory($directory,$acl)
    $script:UiDirectory=$directory
    $path=Join-Path $directory 'worker.ps1'
    # Windows PowerShell 5.1 decodes BOM-less scripts using the system ANSI
    # code page. Preserve the Korean adapter selectors on every guest locale.
    $source=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__UI_WORKER_BASE64__'))
    [IO.File]::WriteAllText($path,$source,(New-Object Text.UTF8Encoding($true)))
    $start=New-Object Diagnostics.ProcessStartInfo
    $start.FileName=Join-Path $PSHOME 'powershell.exe'
    $start.Arguments='-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Mta -File "'+$path+'"'
    $start.UseShellExecute=$false;$start.CreateNoWindow=$true
    $start.RedirectStandardInput=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
    $start.StandardOutputEncoding=New-Object Text.UTF8Encoding($false)
    $script:UiWorker=New-Object Diagnostics.Process
    $script:UiWorker.StartInfo=$start
    Start-MendimaruUiProcess $script:UiWorker
    $script:UiJob=New-Object MendimaruUiJob($script:UiWorker.Handle)
    # Drain initialization diagnostics privately; never forward guest exceptions.
    $script:UiWorker.BeginErrorReadLine()
    Send-MendimaruUiLine $script:UiWorker (ConvertTo-Json -InputObject $script:UiIdentity -Compress)
}
function Write-MendimaruUiResult($Response) {
    $resultPath=$controlPath+'.ui.report'
    $payload=[ordered]@{id=$script:UiPending.id;ok=[bool]$Response.ok}
    if($Response.ok){$payload.data=$Response.data}else{
        $payload.reason=$Response.reason
        if($null -ne $Response.diagnostic){$payload.diagnostic=$Response.diagnostic}
    }
    Write-MendimaruReport $payload
    $script:UiPending=$null;$script:UiTask=$null
}
function Service-MendimaruUi {
    Service-MendimaruSessionQuery
    Service-MendimaruHostService
    if($null -eq $script:UiIdentity){return}
    $closePath=$controlPath+'.ui.close'
    if(Test-Path -LiteralPath $closePath){
        try {
            $authClose=Read-MendimaruAuthenticatedPayload -Path $closePath
            $close=$authClose.Json|ConvertFrom-Json
            if($authClose.Sequence -gt $script:UiSequence -and $close.close -eq $true -and $close.sessionId -ceq $script:UiIdentity.sessionId){
                Remove-Item -LiteralPath $closePath -Force
                Close-MendimaruUi
                $script:UiClosed=$true
                return
            }
        }catch{}
    }
    if($script:UiVerified -and $null -ne $script:UiWorker -and -not [MendimaruUiJob]::Active([int]$script:UiIdentity.interactiveSessionId)){
        Stop-MendimaruUi
        if($null -ne $script:UiPending){Write-MendimaruUiResult @{ok=$false;reason='ui-no-interactive-desktop'};return}
    }
    $requestPath=$controlPath+'.ui.request'
    $cancelPath=$controlPath+'.ui.cancel'
    try {
        if($null -ne $script:UiPending){
            if(Test-Path -LiteralPath $cancelPath){
                try {
                    $auth=Read-MendimaruAuthenticatedPayload -Path $cancelPath
                    $cancel=$auth.Json|ConvertFrom-Json
                    if($cancel.id -ceq $script:UiPending.id -and $cancel.cancel -eq $true){
                        Stop-MendimaruUi;Write-MendimaruUiResult @{ok=$false;reason='ui-cancelled'}
                    }
                    Remove-Item -LiteralPath $cancelPath -Force
                }catch{}
                if($null -eq $script:UiPending){return}
            }
            if([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() -ge ([long]$script:UiPending.expiresAt + $script:UiRequestClockGraceMs)){
                Stop-MendimaruUi;Write-MendimaruUiResult @{ok=$false;reason='ui-helper-timeout'};return
            }
            if($script:UiTask.IsCompleted){
                try {$line=$script:UiTask.GetAwaiter().GetResult();if(-not $line -or $line.Length -gt 12582912){throw 'bad-response'};$result=$line|ConvertFrom-Json}
                catch {Stop-MendimaruUi;$result=@{ok=$false;reason='ui-helper-exited'}}
                if($result.ok){$script:UiVerified=$true}
                if(-not $result.ok -and $result.reason -in @('ui-no-interactive-desktop','ui-session-unavailable')){Stop-MendimaruUi}
                Write-MendimaruUiResult $result
            }
            return
        }
        if(-not (Test-Path -LiteralPath $requestPath)){return}
        try {
            $auth=Read-MendimaruAuthenticatedPayload -Path $requestPath
            if($auth.Sequence -le $script:UiSequence){throw 'replayed-request'}
            $message=$auth.Json|ConvertFrom-Json
            if($message.id -cnotmatch '^ui_[0-9a-f]{32}$' -or $message.request.sessionId -cne $script:UiIdentity.sessionId){throw 'invalid-identity'}
            if($message.request.action -ceq 'set-value'){
                # 기존 envelope HMAC 검증이 성공한 뒤에만 AES-CBC를 복호화한다.
                # 허용된 UI 입력값도 공유 파일에 평문/base64로 남기지 않는다.
                $sealed=$message.request.sealedValue
                if($null -ne $message.request.value -or $sealed.version -ne 1 -or
                    (@($sealed.PSObject.Properties.Name|Sort-Object) -join ',') -cne 'ciphertext,iv,version'){throw 'invalid-confidential-value'}
                $iv=[Convert]::FromBase64String([string]$sealed.iv)
                $cipher=[Convert]::FromBase64String([string]$sealed.ciphertext)
                if($iv.Length -ne 16 -or $cipher.Length -lt 16 -or $cipher.Length -gt 272 -or $cipher.Length % 16 -ne 0){throw 'invalid-confidential-value'}
                $aes=[Security.Cryptography.Aes]::Create()
                $plain=$null;$valueKey=$null;$decryptor=$null
                try {
                    $valueKey=$script:MendimaruHmac.ComputeHash([Text.Encoding]::ASCII.GetBytes('mendimaru-ui-value-v1'))
                    $aes.Key=$valueKey;$aes.IV=$iv;$aes.Mode=[Security.Cryptography.CipherMode]::CBC;$aes.Padding=[Security.Cryptography.PaddingMode]::PKCS7
                    $decryptor=$aes.CreateDecryptor()
                    $plain=$decryptor.TransformFinalBlock($cipher,0,$cipher.Length)
                    $value=(New-Object Text.UTF8Encoding($false,$true)).GetString($plain)
                    if($plain.Length -gt 256 -or $value -match '[\x00-\x1f\x7f]'){throw 'invalid-confidential-value'}
                    $message.request.PSObject.Properties.Remove('sealedValue')
                    $message.request|Add-Member -NotePropertyName value -NotePropertyValue $value
                }finally{
                    if($null -ne $plain){[Array]::Clear($plain,0,$plain.Length)}
                    if($null -ne $valueKey){[Array]::Clear($valueKey,0,$valueKey.Length)}
                    if($null -ne $decryptor){$decryptor.Dispose()};$aes.Dispose()
                }
            }elseif($null -ne $message.request.sealedValue){throw 'invalid-confidential-value'}
            $script:UiSequence=$auth.Sequence
            Remove-Item -LiteralPath $requestPath -Force
            $script:UiPending=$message
        }catch{return}
        if(Test-Path -LiteralPath $cancelPath){
            try {
                $authCancel=Read-MendimaruAuthenticatedPayload -Path $cancelPath
                $cancel=$authCancel.Json|ConvertFrom-Json
                if($cancel.id -ceq $message.id -and $cancel.cancel -eq $true){Remove-Item -LiteralPath $cancelPath -Force;Write-MendimaruUiResult @{ok=$false;reason='ui-cancelled'};return}
            }catch{}
        }
        $now=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        if([long]$message.expiresAt -le $now -or [long]$message.expiresAt -gt ($now+60000+$script:UiRequestClockGraceMs)){Write-MendimaruUiResult @{ok=$false;reason='ui-request-expired'};return}
        if($message.request.operation -ceq 'studio-logs'){
            try { Write-MendimaruUiResult @{ok=$true;data=(Get-MendimaruStudioLogs $message.request.logs)} }
            catch {
                $reason=[string]$_.Exception.Message
                if($reason -cnotin @('studio-logs-not-found','studio-logs-unreadable','studio-logs-wrong-session','studio-logs-unsupported-version','studio-logs-cursor-invalid')){$reason='studio-logs-unreadable'}
                Write-MendimaruUiResult @{ok=$false;reason=$reason}
            };return
        }
        if($message.request.operation -ceq 'release'){
            Stop-MendimaruUi;Write-MendimaruUiResult @{ok=$true;data=@{sessionId=$script:UiIdentity.sessionId;released=$true}};return
        }
        if($message.request.operation -cnotin @('capabilities','tree','find','action','wait','screenshot')){Write-MendimaruUiResult @{ok=$false;reason='ui-invalid-request'};return}
        Start-MendimaruUi
        Send-MendimaruUiLine $script:UiWorker (ConvertTo-Json -InputObject $message -Depth 16 -Compress)
        $script:UiTask=$script:UiWorker.StandardOutput.ReadLineAsync()
    }catch{
        Stop-MendimaruUi
        if($null -ne $script:UiPending){Write-MendimaruUiResult @{ok=$false;reason='ui-provider-failed'}}
    }
}
