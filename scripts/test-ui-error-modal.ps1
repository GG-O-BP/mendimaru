# Real Win32 dialog and real UIA providers. Only the exact Studio identity lookup
# is substituted with a test-owned process; production Scan/Act/guards run intact.
$ErrorActionPreference='Stop'
$text=[IO.File]::ReadAllText((Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\scripts\ui_worker.ps1'))
. ([ScriptBlock]::Create($text.Substring(0,$text.IndexOf('$null = [MendimaruUiNative]::SetThreadDpiAwarenessContext'))))
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseInput($text,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'worker syntax errors'}
foreach($definition in $ast.FindAll({param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst]},$true)){. ([ScriptBlock]::Create($definition.Extent.Text))}
$lab=Join-Path $env:TEMP ('mendimaru-error-modal-'+[Guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $lab
$process=$null
try {
    $fixture=Join-Path $lab 'fixture.ps1'
    [IO.File]::WriteAllText($fixture,@'
param($StatePath)
Add-Type -AssemblyName PresentationFramework
Add-Type -TypeDefinition @"
using System;
using System.IO;
using System.Runtime.InteropServices;
public static class ErrorFixture {
    delegate IntPtr DialogProc(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr DialogBoxIndirectParam(IntPtr i, IntPtr t, IntPtr owner, DialogProc p, IntPtr l);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr CreateWindowEx(uint ex,string c,string text,uint style,int x,int y,int w,int h,IntPtr parent,IntPtr id,IntPtr instance,IntPtr p);
    [DllImport("user32.dll")] static extern bool EndDialog(IntPtr h,IntPtr r);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool SetWindowText(IntPtr h,string text);
    static IntPtr details;
    static DialogProc callback=Proc;
    static IntPtr Proc(IntPtr h,uint m,IntPtr w,IntPtr l) {
        if(m==0x110) {
            CreateWindowEx(0,"Static","Compilation of the app bundle failed.",0x50000000,10,10,340,25,h,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero);
            CreateWindowEx(0,"Button","Details >>",0x50010000,10,45,100,30,h,(IntPtr)101,IntPtr.Zero,IntPtr.Zero);
            CreateWindowEx(0,"Button","Close",0x50010001,240,45,100,30,h,(IntPtr)102,IntPtr.Zero,IntPtr.Zero);
            CreateWindowEx(0,"Edit","FIXTURE-PASSWORD-MUST-NOT-LEAK",0x50000020,10,180,100,20,h,(IntPtr)104,IntPtr.Zero,IntPtr.Zero);
            details=CreateWindowEx(0,"Edit","",0x50800844,10,85,330,90,h,(IntPtr)103,IntPtr.Zero,IntPtr.Zero);
            return (IntPtr)1;
        }
        if(m==0x111) {
            int id=w.ToInt32() & 0xffff;
            if(id==101)SetWindowText(details,"Fixture diagnostics: bundler failed; no credentials. "+new string('x',1024)+" END-OF-DETAILS");
            if(id==102 || id==2)EndDialog(h,IntPtr.Zero);
            return (IntPtr)1;
        }
        if(m==0x10) { EndDialog(h,IntPtr.Zero); return (IntPtr)1; }
        return IntPtr.Zero;
    }
    public static void Show(IntPtr owner) {
        byte[] template;
        using(var stream=new MemoryStream())using(var b=new BinaryWriter(stream)) {
            b.Write((uint)0x80c80080); b.Write((uint)0); b.Write((ushort)0);
            b.Write((short)0);b.Write((short)0);b.Write((short)250);b.Write((short)130);
            b.Write((ushort)0);b.Write((ushort)0);
            foreach(char c in "Error")b.Write((ushort)c);b.Write((ushort)0);template=stream.ToArray();
        }
        var memory=Marshal.AllocHGlobal(template.Length);
        try { Marshal.Copy(template,0,memory,template.Length);DialogBoxIndirectParam(IntPtr.Zero,memory,owner,callback,IntPtr.Zero); }
        finally { Marshal.FreeHGlobal(memory); }
    }
}
"@
$form=New-Object Windows.Window
$form.Title='Mendimaru owned modal fixture'
$form.Width=400;$form.Height=250
$button=New-Object Windows.Controls.Button
$button.Content='Open Error'
$button.Add_Click({[ErrorFixture]::Show((New-Object Windows.Interop.WindowInteropHelper($form)).Handle)})
$form.Content=$button
$form.Add_ContentRendered({[IO.File]::WriteAllText($StatePath,(@{main=(New-Object Windows.Interop.WindowInteropHelper($form)).Handle.ToInt64()}|ConvertTo-Json -Compress))})
$null=$form.ShowDialog()
'@)
    $state=Join-Path $lab 'state.json'
    $process=Start-Process (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile','-STA','-File',('"'+$fixture+'"'),('"'+$state+'"')) -PassThru
    $until=[DateTime]::UtcNow.AddSeconds(20)
    while(-not (Test-Path $state)){if($process.HasExited -or [DateTime]::UtcNow -ge $until){throw 'fixture did not start'};Start-Sleep -Milliseconds 50}
    $started=$process.StartTime.ToUniversalTime().Ticks
    $script:Identity=@{processId=$process.Id;sessionId=('studio-'+$process.Id+'-'+$started);startedTicks=$started.ToString();fileVersion='11.12.6.0';hostPlatform='linux'}
    $script:Deadline=[DateTime]::UtcNow.AddSeconds(60)
    $script:Generation=[Guid]::NewGuid().ToString('N');$script:Elements=@{};$script:ElementSequence=0;$script:Revision=0
    $walker=[Windows.Automation.TreeWalker]::RawViewWalker
    $script:ObservationCache=New-ObservationCache
    function Target { Check-Deadline;$process.Refresh();if($process.HasExited -or $process.StartTime.ToUniversalTime().Ticks -ne $started){throw 'ui-wrong-session'};return $process }
    function Supported-Version {}
    function Await-Tree($Predicate) {
        $until=[DateTime]::UtcNow.AddSeconds(10)
        do {$tree=Tree;if(& $Predicate $tree){return $tree};Start-Sleep -Milliseconds 50}while([DateTime]::UtcNow -lt $until)
        throw 'expected dialog effect was not observed'
    }
    $handles=[IO.File]::ReadAllText($state)|ConvertFrom-Json
    $main=[Windows.Automation.AutomationElement]::FromHandle([IntPtr]$handles.main)
    $condition=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,'Open Error')
    $open=$main.FindFirst([Windows.Automation.TreeScope]::Descendants,$condition)
    if($null -eq $open){throw 'fixture Invoke button missing'}
    $openId=Register $open
    $watch=[Diagnostics.Stopwatch]::StartNew()
    Write-Output ('Fixture open: '+$open.Current.ControlType.ProgrammaticName+' / '+$open.Current.FrameworkId)
    $null=Act @{elementId=$openId;action='invoke'}
    if($watch.ElapsedMilliseconds -gt 1500){throw 'modal Invoke blocked the worker'}
    $tree=Await-Tree {param($t) @($t.root.nodes|Where-Object{$_.name -ceq 'Details >>'}).Count -eq 1}
    $staleRejected=$false
    try {$null=Act @{elementId=$openId;action='invoke'}}catch{$staleRejected=$_.Exception.Message -ceq 'ui-stale-element'}
    if(-not $staleRejected){throw 'lost Invoke response could replay an old observed element'}
    $details=@($tree.root.nodes|Where-Object{$_.name -ceq 'Details >>'})[0]
    $detailsElement=Resolve $details.elementId
    $detailsWindow=Window-For $detailsElement
    $detailsRoot=[Windows.Automation.AutomationElement]::FromHandle($detailsWindow)
    Write-Output ('Fixture details: '+$details.role+' / '+$details.framework+' / '+[MendimaruUiNative]::Class([IntPtr]$detailsElement.Current.NativeWindowHandle)+' / '+$detailsRoot.Current.Name+' / '+$detailsRoot.Current.FrameworkId+' / '+(Native-ErrorWindow $detailsWindow))
    $null=Act @{elementId=$details.elementId;action='invoke'}
    $tree=Await-Tree {param($t) @($t.root.nodes|Where-Object{$_.value -like '*END-OF-DETAILS'}).Count -eq 1}
    if(($tree | ConvertTo-Json -Depth 16) -like '*FIXTURE-PASSWORD-MUST-NOT-LEAK*'){throw 'native password leaked into public tree'}
    $errorWindow=@($tree.root.windows|Where-Object{$_.name -ceq 'Error'})[0]
    $root=Resolve $errorWindow.elementId
    $null=[MendimaruUiNative]::SetForegroundWindow([IntPtr][long]$errorWindow.handle)
    $null=Act @{elementId=$errorWindow.elementId;action='keyboard-input';value='Tab'}
    # The original owner remains disabled: no second invocation may cross it.
    $rejected=$false
    try {$null=Input-Guard $open $false}catch{$rejected=$_.Exception.Message -in @('ui-unsupported-element','ui-modal-blocked')}
    if(-not $rejected){throw 'disabled owner accepted duplicate input'}
    $tree=Tree
    $close=@($tree.root.nodes|Where-Object{$_.name -ceq 'Close'})[0]
    Write-Output ('Fixture close: '+$close.role+' / '+$close.framework)
    $null=Act @{elementId=$close.elementId;action='invoke'}
    $null=Await-Tree {param($t) @($t.root.windows|Where-Object{$_.name -ceq 'Error'}).Count -eq 0}
    Write-Output 'UIA Invoke -> native Error -> Details -> bounded read -> keyboard Tab -> Close: PASS'
} finally {
    if($process){if(-not $process.HasExited){$process.Kill();$null=$process.WaitForExit(5000)};$process.Dispose()}
    Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue
}
