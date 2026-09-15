# Included in the authenticated Studio launch/reconnect host. Worker source is
# embedded in the hash-verified launch script; requests can never replace it.
$script:UiWorker = $null
$script:UiJob = $null
$script:UiTask = $null
$script:UiPending = $null
$script:UiDirectory = $null
$script:UiIdentity = $null
$script:UiSequence = [long]0
$script:UiVerified = $false

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
    $null=$script:UiWorker.Start()
    $script:UiJob=New-Object MendimaruUiJob($script:UiWorker.Handle)
    # Drain initialization diagnostics privately; never forward guest exceptions.
    $script:UiWorker.BeginErrorReadLine()
    $script:UiWorker.StandardInput.WriteLine((ConvertTo-Json -InputObject $script:UiIdentity -Compress))
}
function Write-MendimaruUiResult($Response) {
    $resultPath=$controlPath+'.ui.report'
    $payload=[ordered]@{id=$script:UiPending.id;ok=[bool]$Response.ok}
    if($Response.ok){$payload.data=$Response.data}else{$payload.reason=$Response.reason}
    Write-MendimaruReport $payload
    $script:UiPending=$null;$script:UiTask=$null
}
function Service-MendimaruUi {
    if($null -eq $script:UiIdentity){return}
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
            if([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() -ge [long]$script:UiPending.expiresAt){
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
        if([long]$message.expiresAt -le $now -or [long]$message.expiresAt -gt $now+60000){Write-MendimaruUiResult @{ok=$false;reason='ui-request-expired'};return}
        if($message.request.operation -ceq 'release'){
            Stop-MendimaruUi;Write-MendimaruUiResult @{ok=$true;data=@{sessionId=$script:UiIdentity.sessionId;released=$true}};return
        }
        if($message.request.operation -cnotin @('capabilities','tree','find','action','wait','screenshot')){Write-MendimaruUiResult @{ok=$false;reason='ui-invalid-request'};return}
        Start-MendimaruUi
        $script:UiWorker.StandardInput.WriteLine($auth.Json)
        $script:UiTask=$script:UiWorker.StandardOutput.ReadLineAsync()
    }catch{
        Stop-MendimaruUi
        if($null -ne $script:UiPending){Write-MendimaruUiResult @{ok=$false;reason='ui-provider-failed'}}
    }
}
