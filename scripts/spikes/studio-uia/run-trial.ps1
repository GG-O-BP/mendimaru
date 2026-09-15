param(
    [Parameter(Mandatory=$true)][string]$LabRoot,
    [Parameter(Mandatory=$true)][string]$LabShare,
    [Parameter(Mandatory=$true)][ValidateSet(10,11)][int]$Major,
    [Parameter(Mandatory=$true)][ValidateRange(1,20)][int]$Trial,
    [Parameter(Mandatory=$true)][ValidateSet('cold','warm')][string]$Phase,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-z][a-z0-9-]{0,39}$')][string]$CampaignId
)
# Experimental, fixture-specific harness, not a product provider.
# Requires the private disposable lab described in README.md and an external
# process supervisor. A PowerShell loop cannot interrupt a blocked UIA COM call.
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
if([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT){throw 'windows-guest-required'}
if($LabRoot -notmatch '^[A-Za-z]:\\'){throw 'local-lab-required'}
$LabRoot=[IO.Path]::GetFullPath($LabRoot).TrimEnd('\')
$OutName='campaign-'+$CampaignId+'-'+$Major+'-'+$Trial.ToString('00')
if((Test-Path "$LabRoot\$OutName") -or (Test-Path "$LabShare\$OutName.json")){throw 'trial-already-exists'}
$expectedVersion=if($Major -eq 10){'10.24.26.0'}else{'11.12.4.0'}
$idCheck=[IO.File]::ReadAllText("$LabRoot\studio$Major.json")|ConvertFrom-Json
if($idCheck.version -cne $expectedVersion -or $idCheck.project -cne "$LabRoot\project$Major\UIA16_10.mpr"){throw 'fixture-identity-mismatch'}
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes,WindowsBase,System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices; using System.Text;
public class U16 {
 [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint x,uint y,uint d,UIntPtr e);
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
 [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h,uint f);
 [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetDpiForSystem();
 [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint f,bool i,uint a);
 [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr h);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr h,int i,StringBuilder v,uint l,out uint n);
 [DllImport("wtsapi32.dll")] static extern bool WTSQuerySessionInformation(IntPtr server,int session,int info,out IntPtr buffer,out int size);
 [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr buffer);
 public static bool DefaultDesktop(){var d=OpenInputDesktop(0,false,1);if(d==IntPtr.Zero)return false;try{var b=new StringBuilder(256);uint n;return GetUserObjectInformation(d,2,b,512,out n)&&b.ToString()=="Default";}finally{CloseDesktop(d);}}
 public static int SessionState(int session){IntPtr p;int n;if(!WTSQuerySessionInformation(IntPtr.Zero,session,8,out p,out n))return -1;try{return n>=4?Marshal.ReadInt32(p):-1;}finally{WTSFreeMemory(p);}}

}
'@
$null=[U16]::SetThreadDpiAwarenessContext([IntPtr](-4))
$script:Id=$null
function Root {
 $p=Get-Process -Id $script:Id.pid
 if(-not [U16]::DefaultDesktop()){throw 'no-interactive-desktop'}
 if($p.SessionId -eq 0 -or [U16]::SessionState($p.SessionId) -ne 0){throw 'session-not-active'}
 if($p.ProcessName -cne 'studiopro' -or $p.Path -cne $script:Id.exe){throw 'wrong-executable'}
 if($p.StartTime.ToUniversalTime().Ticks.ToString() -ne $script:Id.startTicks -or $p.MainModule.FileVersionInfo.FileVersion -ne $script:Id.version -or $p.SessionId -ne (Get-Process -Id $PID).SessionId){throw 'identity-mismatch'}
 if($p.MainWindowHandle -eq [IntPtr]::Zero){throw 'no-main-window'}
 [Windows.Automation.AutomationElement]::FromHandle($p.MainWindowHandle)
}
function Find([string]$Name,[string]$Type='',[Windows.Automation.AutomationElement]$Under=$null){
 if($null -eq $Under){$Under=Root}
 $a=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,$Name)
 if($Type){$b=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::$Type);$a=New-Object Windows.Automation.AndCondition($a,$b)}
 $raw=$Under.FindAll([Windows.Automation.TreeScope]::Descendants,$a);$unique=@{};foreach($v in $raw){$unique[($v.GetRuntimeId() -join '.') ]=$v};$e=@($unique.Values)
 if($e.Count -ne 1){throw "locator-$Name-count-$($e.Count)"};return $e[0]
}
function IdFind([string]$Name){$a=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::AutomationIdProperty,$Name);$e=(Root).FindAll([Windows.Automation.TreeScope]::Descendants,$a);if($e.Count -ne 1){throw "id-$Name-count-$($e.Count)"};return $e[0]}
function Click($e,$within=$null){
 $r=Root;$h=[IntPtr]$r.Current.NativeWindowHandle
 if([U16]::IsIconic($h)){throw 'minimized'}
 if(-not [U16]::IsWindowEnabled($h)){throw 'blocked-by-modal'}
 $null=[U16]::SetForegroundWindow($h);Start-Sleep -Milliseconds 100
 if([U16]::GetForegroundWindow() -ne $h){throw 'foreground-lost'}
 $b=$e.Current.BoundingRectangle;$rb=$r.Current.BoundingRectangle
 if($null -ne $within){$b=[Windows.Rect]::Intersect($b,$within.Current.BoundingRectangle)}
 $b=[Windows.Rect]::Intersect($b,$rb)
 $screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds;$b=[Windows.Rect]::Intersect($b,(New-Object Windows.Rect($screen.X,$screen.Y,$screen.Width,$screen.Height)))
 if($e.Current.IsOffscreen -or -not $e.Current.IsEnabled -or $b.Width -le 0 -or $b.Height -le 0){throw 'not-clickable'}
 $pt=New-Object U16+POINT;$pt.X=[int]($b.X+$b.Width/2);$pt.Y=[int]($b.Y+$b.Height/2)
 if(-not $rb.Contains($pt.X,$pt.Y)){throw 'outside-root'}
 if([U16]::GetAncestor([U16]::WindowFromPoint($pt),2) -ne $h){throw 'occluded'}
 $null=[U16]::SetCursorPos($pt.X,$pt.Y);[U16]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[U16]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Start-Sleep -Milliseconds 300
}
function Invoke($e){$p=$null;if(-not $e.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern,[ref]$p)){throw 'invoke-unsupported'};$p.Invoke();Start-Sleep -Milliseconds 300}
function Keys([string]$keys){$r=Root;$h=[IntPtr]$r.Current.NativeWindowHandle;$null=[U16]::SetForegroundWindow($h);Start-Sleep -Milliseconds 100;if([U16]::GetForegroundWindow() -ne $h){throw 'foreground-lost'};[Windows.Forms.SendKeys]::SendWait($keys);Start-Sleep -Milliseconds 300}
function NameEdit {
 $scope=Find 'Properties' 'TabItem';$a=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,'Name')
 $b=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Text)
 $labels=$scope.FindAll([Windows.Automation.TreeScope]::Descendants,(New-Object Windows.Automation.AndCondition($a,$b)))
 $w=[Windows.Automation.TreeWalker]::RawViewWalker;$candidates=@{}
 foreach($label in $labels){
  $e=$label
  for($i=0;$i -lt 4;$i++){
   $e=$w.GetParent($e);$a=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)
   $ed=$e.FindAll([Windows.Automation.TreeScope]::Descendants,$a)
   if($ed.Count -eq 1){$candidates[($ed[0].GetRuntimeId() -join '.')]=$ed[0];break}
   if($ed.Count -gt 1){break}
  }
 }
 if($candidates.Count -ne 1){throw ('name-editor-count-'+$candidates.Count)}
 return @($candidates.Values)[0]
}
function Value($e){([Windows.Automation.ValuePattern]$e.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).Current.Value}

if($Phase -eq 'cold'){
$script:Id=[IO.File]::ReadAllText("$LabRoot\studio$Major.json")|ConvertFrom-Json
$launchClock=[Diagnostics.Stopwatch]::StartNew();$previous=$script:Id
$old=Get-Process -Id $previous.pid -ErrorAction SilentlyContinue
if($null -ne $old -and $old.StartTime.ToUniversalTime().Ticks.ToString() -ceq $previous.startTicks){
 $null=Root
 $rt=@(Get-CimInstance Win32_Process -Filter "Name = 'javaw.exe'"|Where-Object{$_.ParentProcessId -eq $old.Id})
 if($rt.Count -gt 0){throw 'runtime-present-before-cold-start'}
 (Find 'File' 'MenuItem').SetFocus();Keys '^+s'
 if(-not $old.CloseMainWindow()){throw 'close-not-delivered'}
 if(-not $old.WaitForExit(30000)){throw 'studio-close-timeout'}
}
if([Diagnostics.FileVersionInfo]::GetVersionInfo($previous.exe).FileVersion -cne $previous.version){throw 'launch-version-mismatch'}
$project="$LabRoot\project$Major\UIA16_10.mpr"
$p=Start-Process -FilePath $previous.exe -ArgumentList ('"'+$project+'"') -PassThru
Start-Sleep -Milliseconds 500
$script:Id=@{pid=$p.Id;startTicks=$p.StartTime.ToUniversalTime().Ticks.ToString();version=$previous.version;session=$p.SessionId;exe=$previous.exe;project=$project;launchUtc=[DateTime]::UtcNow.ToString('o')}
$script:Id|ConvertTo-Json|Set-Content "$LabRoot\studio$Major.json" -Encoding UTF8
Copy-Item "$LabRoot\studio$Major.json" "$LabShare\studio$Major.json" -Force
$clock=[Diagnostics.Stopwatch]::StartNew();$skippedSignIn=$false;$ready=$false
while($clock.ElapsedMilliseconds -lt 120000){
 try{
  $r=Root
  if($r.Current.Name -like 'UIA16_10*'){$null=Find 'Welcome' 'TabItem';$null=Find 'File' 'MenuItem';$ready=$true;break}
  if(-not $skippedSignIn){try{$b=Find 'Sign In Later' 'Button';Invoke $b;$skippedSignIn=$true}catch{}}
 }catch{}
 Start-Sleep -Milliseconds 500
}
if(-not $ready){throw 'studio-ready-timeout'}
$notificationClosed=$false
try{
 $n=Find 'Notification Stack' 'Window'
 $c=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,'Microsoft Defender affects performance')
 if($n.FindAll([Windows.Automation.TreeScope]::Descendants,$c).Count -gt 0){([Windows.Automation.WindowPattern]$n.GetCurrentPattern([Windows.Automation.WindowPattern]::Pattern)).Close();$notificationClosed=$true}
}catch{}
@{previousPid=$previous.pid;previousStartTicks=$previous.startTicks;pid=$script:Id.pid;startTicks=$script:Id.startTicks;version=$script:Id.version;elapsedMs=$launchClock.ElapsedMilliseconds;signInLater=$skippedSignIn;defenderNotificationClosed=$notificationClosed;utc=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json|Set-Content "$LabShare\$OutName-launch.json" -Encoding UTF8

}
# One representative flow. Failed steps and cleanup results are retained.
$script:Id=[IO.File]::ReadAllText("$LabRoot\studio$Major.json")|ConvertFrom-Json
$fixture="$LabRoot\project$Major";$out="$LabRoot\$OutName"
if(Test-Path $out){throw 'output-already-exists'}
$null=New-Item -ItemType Directory $out
$events=New-Object Collections.Generic.List[object]
$trialName='uia16T'+$Major+'_'+$Trial.ToString('00')
$stepOrder=@('open-page','select-widget','change-property','structure-mode','design-mode','synchronize-f4','run-locally')
$active='open-page';$watch=[Diagnostics.Stopwatch]::StartNew();$failed=$false
function Emit {
 @{version=$script:Id.version;pid=$script:Id.pid;startTicks=$script:Id.startTicks;phase=$Phase;trial=$Trial;trialName=$trialName;active=$active;steps=$events.ToArray();utc=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json -Depth 10|Set-Content "$out\flow.json" -Encoding UTF8
 Copy-Item "$out\flow.json" "$LabShare\$OutName.json" -Force
}
function Step([string]$name,[scriptblock]$body){
 $script:active=$name;$clock=[Diagnostics.Stopwatch]::StartNew();Emit
 try{$proof=& $body;$events.Add(@{id=$name;status='pass';elapsedMs=$clock.ElapsedMilliseconds;proof=$proof})}
 catch{$events.Add(@{id=$name;status='fail';elapsedMs=$clock.ElapsedMilliseconds;error=[string]$_.Exception.Message});$script:failed=$true;Emit;throw}
 Emit
}
function NativeFocus { (Find 'File' 'MenuItem').SetFocus() }
function WaitDoc([int]$seconds=20){
 $clock=[Diagnostics.Stopwatch]::StartNew()
 do{try{return Find 'MyFirstModule.Home_Web' 'Document'}catch{if($clock.ElapsedMilliseconds -ge $seconds*1000){throw};Start-Sleep -Milliseconds 400}}while($true)
}
function SetSearch([string]$value){
 $searchName=if($Major -eq 11){'Search'+[char]0x2026}else{'Search...'};$ed=Find $searchName 'Edit' (Find 'Toolbox' 'TabItem');$vp=[Windows.Automation.ValuePattern]$ed.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)
 if($vp.Current.IsReadOnly){throw 'readonly-search'};$vp.SetValue($value);Start-Sleep -Milliseconds 400
}
function RatingCount {
 $doc=Find 'Toolbox' 'TabItem'
 $a=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,'Rating')
 $kind=if($Major -eq 11){[Windows.Automation.ControlType]::ListItem}else{[Windows.Automation.ControlType]::DataItem}
 $b=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,$kind)
 $c=New-Object Windows.Automation.AndCondition($a,$b)
 return $doc.FindAll([Windows.Automation.TreeScope]::Descendants,$c).Count
}
function CheckRating([int]$expected){
 $clock=[Diagnostics.Stopwatch]::StartNew()
 do { $count=RatingCount;if($count -eq $expected){return $count};if($clock.ElapsedMilliseconds -ge 20000){throw 'rating-state-timeout'};Start-Sleep -Milliseconds 400 }while($true)
}
function Snapshot([string]$name){
 $env:WINAPP_CLI_TELEMETRY_OPTOUT='1'
 $old=$ErrorActionPreference;$ErrorActionPreference='Continue'
 try{& "$LabRoot\winapp\winapp.exe" ui inspect -a $script:Id.pid --depth 40 --json > "$out\$name.json" 2> "$out\$name.stderr";$code=$LASTEXITCODE}finally{$ErrorActionPreference=$old}
 if($code -ne 0){throw 'snapshot-failed'}
}
try{
 $r=Root
 ([Windows.Automation.WindowPattern]$r.GetCurrentPattern([Windows.Automation.WindowPattern]::Pattern)).SetWindowVisualState([Windows.Automation.WindowVisualState]::Maximized)
 Start-Sleep -Milliseconds 300;$r=Root
 $screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds
 if($screen.Width -ne 1280 -or $screen.Height -ne 800 -or [U16]::GetDpiForSystem() -ne 96){throw 'environment-not-frozen'}
 @{session=$script:Id.session;dpi=[U16]::GetDpiForWindow([IntPtr]$r.Current.NativeWindowHandle);systemDpi=[U16]::GetDpiForSystem();width=$screen.Width;height=$screen.Height;desktop='Default';wtsState=[U16]::SessionState($script:Id.session);version=$script:Id.version;pid=$script:Id.pid;startTicks=$script:Id.startTicks}|ConvertTo-Json|Set-Content "$out\preflight.json" -Encoding UTF8
 if($r.Current.Name -notlike 'UIA16_10*'){throw 'wrong-fixture-window'}
 Step 'open-page' {
  NativeFocus;Click (Find 'Welcome' 'TabItem')
  NativeFocus;Keys '^g'
  $dialog=Find 'Go To' 'Window'
  $cond=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)
  $ed=$dialog.FindAll([Windows.Automation.TreeScope]::Descendants,$cond)
  if($ed.Count -ne 1){throw 'ambiguous-go-to-search'}
  ([Windows.Automation.ValuePattern]$ed[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).SetValue('Home_Web');Start-Sleep -Milliseconds 400
  Invoke (Find 'Go To' 'Button' $dialog)
  Invoke (Find 'Design mode' 'Button')
  $doc=WaitDoc
  $uri=Value $doc
  if($uri -notmatch '[?&]theme=Light&' -or $uri -notmatch '[?&]lng=en-US&'){throw 'studio-presentation-changed'}
  @{document=[string]$doc.Current.Name;framework=[string]$doc.Current.FrameworkId;theme='Light';language='en-US'}
 }
 Step 'select-widget' {
  $doc=WaitDoc
  $c=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Text)
  $bodyMatches=@($doc.FindAll([Windows.Automation.TreeScope]::Descendants,$c)|Where-Object{$_.Current.Name.StartsWith('Getting started:')})
  if($bodyMatches.Count -ne 1){throw 'ambiguous-preview-body'}
  Click $bodyMatches[0] $doc;NativeFocus;Click (Find 'Properties' 'TabItem')
  $value=Value (NameEdit)
  if($value -notmatch '^uia16(Probe|T(10|11)_\d{2})$'){throw 'wrong-widget-selected'}
  @{name=$value;locator='page-document/text-prefix + properties-name-row'}
 }
 Step 'change-property' {
  $ed=NameEdit;$before=Value $ed;$oldId=$ed.GetRuntimeId() -join '.'
  if($before -eq $trialName){throw 'trial-value-already-present'}
  $pattern=[Windows.Automation.ValuePattern]$ed.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)
  if($pattern.Current.IsReadOnly){throw 'readonly-property'}
  $pattern.SetValue($trialName);$ed.SetFocus();Keys '{TAB}'
  $new=NameEdit;$after=Value $new
  if($after -cne $trialName){throw 'property-readback-mismatch'}
  NativeFocus;Keys '^+s'
  Snapshot 'property'
  @{before=$before;after=$after;previousRuntimeId=$oldId;readbackRuntimeId=($new.GetRuntimeId() -join '.');mprSha256=(Get-FileHash "$fixture\UIA16_10.mpr" -Algorithm SHA256).Hash.ToLowerInvariant()}
 }
 Step 'structure-mode' {
  Invoke (Find 'Structure mode' 'Button');$b=Find 'Design mode' 'Button'
  if($b.Current.FrameworkId -ne 'WPF'){throw 'structure-verifier-not-supported'}
  Snapshot 'structure'
  @{mode='Structure';toolbarFramework=[string]$b.Current.FrameworkId}
 }
 Step 'design-mode' {
  Invoke (Find 'Design mode' 'Button');$doc=WaitDoc
  @{mode='Design';document=[string]$doc.Current.Name;framework=[string]$doc.Current.FrameworkId}
 }
 Step 'synchronize-f4' {
  NativeFocus;Click (Find 'Toolbox' 'TabItem');SetSearch 'Rating';$before=CheckRating 1
  $src="$fixture\widgets\StarRating.mpk";$held="$out\StarRating.mpk.held";$hash=(Get-FileHash $src -Algorithm SHA256).Hash
  Move-Item $src $held
  try{
   NativeFocus;Keys '{F4}';Start-Sleep -Seconds 3
   NativeFocus;Click (Find 'Toolbox' 'TabItem');SetSearch 'Rating';$removed=CheckRating 0;Snapshot 'f4-removed'
  }finally{if(Test-Path $held){Move-Item $held $src}}
  if((Get-FileHash $src -Algorithm SHA256).Hash -ne $hash){throw 'package-restore-hash-mismatch'}
  NativeFocus;Keys '{F4}';Start-Sleep -Seconds 3
  NativeFocus;Click (Find 'Toolbox' 'TabItem');SetSearch 'Rating';$restored=CheckRating 1;Snapshot 'f4-restored'
  @{before=$before;removed=$removed;restored=$restored;packageSha256=$hash.ToLowerInvariant()}
 }
 Step 'run-locally' {
  $existing=@(Get-CimInstance Win32_Process -Filter "Name = 'javaw.exe'"|Where-Object {$_.ParentProcessId -eq $script:Id.pid})
  if($existing.Count -ne 0){throw 'runtime-already-running'}
  NativeFocus;Keys '{F5}';$clock=[Diagnostics.Stopwatch]::StartNew();$ready=$false
  while($clock.ElapsedMilliseconds -lt 240000){
   try{$r=Invoke-WebRequest 'http://127.0.0.1:8080/' -UseBasicParsing -TimeoutSec 2;if($r.StatusCode -eq 200){$ready=$true;break}}catch{}
   Start-Sleep -Seconds 1
  }
  if(-not $ready){Snapshot 'runtime-not-ready';throw 'runtime-ready-timeout'}
  $runtime=@(Get-CimInstance Win32_Process -Filter "Name = 'javaw.exe'"|Where-Object {$_.ParentProcessId -eq $script:Id.pid -and $_.CommandLine.Contains($fixture+'\deployment')})
  if($runtime.Count -ne 1){throw 'runtime-owner-not-unique'}
  $listen=@(Get-NetTCPConnection -LocalPort 8080 -State Listen|Where-Object {$_.OwningProcess -eq $runtime[0].ProcessId})
  if($listen.Count -lt 1){throw 'runtime-listener-owner-mismatch'}
  Snapshot 'runtime-ready'
  @{http=200;runtimePid=[int]$runtime[0].ProcessId;parentPid=[int]$runtime[0].ParentProcessId;port=8080;readyAfterMs=$clock.ElapsedMilliseconds}
 }
}catch{
 if(-not $failed){$events.Add(@{id=$active;status='fail';elapsedMs=$watch.ElapsedMilliseconds;error=[string]$_.Exception.Message})}
}finally{
 $active='cleanup';Emit
 $runtime=@(Get-CimInstance Win32_Process -Filter "Name = 'javaw.exe'"|Where-Object {$_.ParentProcessId -eq $script:Id.pid})
 if($runtime.Count -eq 1){
  try{
   NativeFocus;Click (Find 'Console' 'TabItem');Invoke (Find 'Stop' 'Button')
   $clock=[Diagnostics.Stopwatch]::StartNew()
   while((Get-Process -Id $runtime[0].ProcessId -ErrorAction SilentlyContinue) -and $clock.ElapsedMilliseconds -lt 30000){Start-Sleep -Milliseconds 500}
   @{runtimeStopped=(-not [bool](Get-Process -Id $runtime[0].ProcessId -ErrorAction SilentlyContinue))}|ConvertTo-Json|Set-Content "$out\cleanup.json" -Encoding UTF8
  }catch{@{runtimeStopped=$false;error=[string]$_.Exception.Message}|ConvertTo-Json|Set-Content "$out\cleanup.json" -Encoding UTF8}
 }else{@{runtimeCount=$runtime.Count}|ConvertTo-Json|Set-Content "$out\cleanup.json" -Encoding UTF8}
 $active='complete';Emit
 Copy-Item $out "$LabShare\$OutName" -Recurse
}
