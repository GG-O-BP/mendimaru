# Private, persistent MTA child. Only the authenticated parent writes stdin.
# No request can name a script, process, executable, or arbitrary coordinates.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
[Console]::InputEncoding = New-Object Text.UTF8Encoding($false)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase, System.Drawing, System.Windows.Forms
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class MendimaruUiNative {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint f, bool i, uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr d);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr h,int i,StringBuilder v,uint n,out uint needed);
    [DllImport("wtsapi32.dll")] static extern bool WTSQuerySessionInformation(IntPtr s,int id,int c,out IntPtr p,out int n);
    [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr p);
    public static bool Active(int id) {
        IntPtr p; int n;
        if (!WTSQuerySessionInformation(IntPtr.Zero,id,8,out p,out n)) return false;
        try { return n >= 4 && Marshal.ReadInt32(p) == 0; } finally { WTSFreeMemory(p); }
    }
    public static bool Desktop() {
        var d=OpenInputDesktop(0,false,1); if(d==IntPtr.Zero)return false;
        try { var b=new StringBuilder(256); uint n; return GetUserObjectInformation(d,2,b,512,out n) && b.ToString()=="Default"; }
        finally { CloseDesktop(d); }
    }
    public static IntPtr[] Windows(uint pid) {
        var windows=new List<IntPtr>();
        EnumWindows((h,p)=>{ uint owner; GetWindowThreadProcessId(h,out owner);
            if(owner==pid && IsWindowVisible(h))windows.Add(h); return windows.Count < 17;
        },IntPtr.Zero); return windows.ToArray();
    }
}
'@
$null = [MendimaruUiNative]::SetThreadDpiAwarenessContext([IntPtr](-4))
$script:Identity = [Console]::ReadLine() | ConvertFrom-Json
$script:Generation = [Guid]::NewGuid().ToString('N')
$script:Elements = @{}
$script:Revision = [long]0
$script:ElementSequence = [long]0
$script:MainHandle = [IntPtr]::Zero
$script:Deadline = [DateTime]::UtcNow
$walker = [Windows.Automation.TreeWalker]::RawViewWalker

function Check-Deadline {
    if ([DateTime]::UtcNow -ge $script:Deadline) { throw 'ui-helper-timeout' }
}
function Target {
    Check-Deadline
    try { $p = Get-Process -Id ([int]$script:Identity.processId) -ErrorAction Stop } catch { throw 'ui-session-unavailable' }
    if ($p.ProcessName -ine 'studiopro' -or $p.Path -ine $script:Identity.executable -or
        $p.StartTime.ToUniversalTime().Ticks.ToString() -cne $script:Identity.startedTicks -or
        $p.MainModule.FileVersionInfo.FileVersion -cne $script:Identity.fileVersion) { throw 'ui-wrong-session' }
    if ($p.SessionId -eq 0 -or $p.SessionId -ne (Get-Process -Id $PID).SessionId) { throw 'ui-wrong-session' }
    if (-not [MendimaruUiNative]::Active($p.SessionId) -or -not [MendimaruUiNative]::Desktop()) { throw 'ui-no-interactive-desktop' }
    if ($p.MainWindowHandle -eq [IntPtr]::Zero) { throw 'ui-session-unavailable' }
    if ($script:MainHandle -ne $p.MainWindowHandle) {
        $script:Generation = [Guid]::NewGuid().ToString('N'); $script:Elements = @{}; $script:MainHandle = $p.MainWindowHandle
    }
    return $p
}
function Supported-Version {
    if ($script:Identity.fileVersion -cnotin @('10.24.26.0','11.12.4.0')) { throw 'ui-unsupported-version' }
}
function Root {
    $p = Target
    return [Windows.Automation.AutomationElement]::FromHandle($p.MainWindowHandle)
}
function Windows {
    $p = Target
    $handles = @([MendimaruUiNative]::Windows([uint32]$p.Id))
    if ($handles.Count -gt 16) { throw 'ui-tree-truncated' }
    return $handles
}
function Window-For($element) {
    # Walk up through WebView descendants, whose renderer PID may differ.
    $current = $element
    for ($n=0; $n -lt 64 -and $null -ne $current; $n++) {
        Check-Deadline
        $handle = [IntPtr]$current.Current.NativeWindowHandle
        if ($handle -ne [IntPtr]::Zero) {
            $top = [MendimaruUiNative]::GetAncestor($handle,2)
            if (@(Windows) -contains $top) { return $top }
        }
        $current = $walker.GetParent($current)
    }
    throw 'ui-stale-element'
}
function Register($element) {
    $c = $element.Current
    $rid = $element.GetRuntimeId() -join '.'
    $script:ElementSequence++
    $id = $script:Generation + ':' + $script:ElementSequence
    $observedValue=$null;$pattern=$null
    if(-not $c.IsPassword -and $element.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern,[ref]$pattern)){$observedValue=$pattern.Current.Value}
    if ($script:Elements.Count -ge 4096 -and -not $script:Elements.ContainsKey($id)) { throw 'ui-tree-truncated' }
    $script:Elements[$id] = @{element=$element; rid=$rid; name=$c.Name; role=$c.ControlType.ProgrammaticName; password=$c.IsPassword; value=$observedValue}
    return $id
}
function Resolve([string]$id) {
    $null = Target
    if (-not $id.StartsWith($script:Generation + ':') -or -not $script:Elements.ContainsKey($id)) { throw 'ui-stale-element' }
    $stored = $script:Elements[$id]
    try {
        $e = $stored.element; $c = $e.Current
        if (($e.GetRuntimeId() -join '.') -cne $stored.rid -or $c.Name -cne $stored.name -or $c.ControlType.ProgrammaticName -cne $stored.role -or $c.IsPassword -ne $stored.password) { throw 'ui-stale-element' }
        $pattern=$null
        if($null -ne $stored.value -and ($e.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern,[ref]$pattern)) -and $pattern.Current.Value -cne $stored.value){throw 'ui-stale-element'}
        $null = Window-For $e
        return $e
    } catch { throw 'ui-stale-element' }
}
function Public-Element($e) {
    $c=$e.Current
    return @{elementId=(Register $e); role=$c.ControlType.ProgrammaticName.Replace('ControlType.',''); name=$(if($c.IsPassword){'[password-redacted]'}else{Short $c.Name})}
}
function Short([string]$s) { return $s.Substring(0,[Math]::Min(256,$s.Length)) }
function Matches($e,$selector) {
    $c=$e.Current
    if ($c.IsPassword) { return $false }
    if ($selector.role -and $c.ControlType.ProgrammaticName -cne ('ControlType.'+$selector.role)) { return $false }
    if ($null -ne $selector.name -and $c.Name -cne $selector.name) { return $false }
    if ($null -ne $selector.automationId -and $c.AutomationId -cne $selector.automationId) { return $false }
    return $true
}
function Scan($scope=$null) {
    # A new observation invalidates the previous element generation. A scope
    # is resolved before scanning, so no stale handle can cross generations.
    $script:Generation=[Guid]::NewGuid().ToString('N');$script:Elements=@{}
    $queue=New-Object Collections.Generic.Queue[object]
    if ($null -ne $scope) { $queue.Enqueue(@{e=$scope;depth=0;parent=$null}) }
    else { foreach($h in @(Windows)){ $queue.Enqueue(@{e=[Windows.Automation.AutomationElement]::FromHandle($h);depth=0;parent=$null}) } }
    $nodes=New-Object Collections.Generic.List[object]; $seen=@{}; $truncated=$false
    while($queue.Count -gt 0) {
        Check-Deadline
        if($nodes.Count -ge 3000){$truncated=$true;break}
        $item=$queue.Dequeue(); $e=$item.e; $rid=$e.GetRuntimeId() -join '.'
        if($seen.ContainsKey($rid)){continue};$seen[$rid]=$true
        $id=Register $e
        $nodes.Add(@{e=$e;id=$id;parent=$item.parent;depth=$item.depth})
        $child=$walker.GetFirstChild($e)
        if($item.depth -ge 48){if($null -ne $child){$truncated=$true};continue}
        while($null -ne $child){
            Check-Deadline
            if($nodes.Count+$queue.Count -ge 3000){$truncated=$true;break}
            $queue.Enqueue(@{e=$child;depth=$item.depth+1;parent=$id});$child=$walker.GetNextSibling($child)
        }
    }
    return @{nodes=$nodes;truncated=$truncated}
}
function Find-Elements($selector,$scope=$null) {
    if($null -eq $scope -and $selector.scopeId){$scope=Resolve $selector.scopeId}
    $scan=Scan $scope
    if($scan.truncated){throw 'ui-tree-truncated'}
    $found=New-Object Collections.Generic.List[object]
    foreach($node in $scan.nodes){if(Matches $node.e $selector){$found.Add($node.e)}}
    return $found.ToArray()
}
function Modal($e) {
    $pattern=$null
    return $e.TryGetCurrentPattern([Windows.Automation.WindowPattern]::Pattern,[ref]$pattern) -and $pattern.Current.IsModal
}
function State($scan) {
    $state='unknown';$dialogs=New-Object Collections.Generic.List[object]
    $statusTexts=New-Object Collections.Generic.List[string];$projectReady=$false;$projectOpen=$false;$busy=$null;$running=$false
    $parents=@{};foreach($node in $scan.nodes){$parents[$node.id]=$node}
    foreach($node in $scan.nodes){
        $c=$node.e.Current
        if($node.depth -eq 0){
            $modal=Modal $node.e;$kind='unknown';$name=$c.Name
            if($name -match '(?i)^(sign.?in|log.?in|로그인)$|^(Mendix Studio Pro|멘딕스 스튜디오 프로).*?(sign.?in|log.?in|로그인)'){$kind='login'}
            elseif($modal -and $name -match '(?i)convert|upgrade|변환'){$kind='conversion'}
            elseif($modal -and $name -match '(?i)update|업데이트'){$kind='update'}
            if($modal -or $kind -eq 'login'){$dialogs.Add(@{elementId=$node.id;kind=$kind;name=(Short $name);modal=$modal})}
        }
        if($c.IsEnabled -and -not $c.IsOffscreen){
            if($c.FrameworkId -ceq 'WPF' -and $c.ControlType -eq [Windows.Automation.ControlType]::DataItem -and $c.Name -match "^(App|앱) '"){$projectOpen=$true}
            if($c.FrameworkId -ceq 'WPF' -and $node.depth -le 8 -and $c.ControlType -eq [Windows.Automation.ControlType]::Text){
                if($c.Name -in @('Ready','준비')){$projectReady=$true}
                $phase=$null
                if($c.Name -match '^(Building|Compiling|빌드 중|컴파일 중)'){$phase='building'}
                elseif($c.Name -match '^(Deploying|배포 중)'){$phase='deploying'}
                elseif($c.Name -match '^(Starting runtime|Starting the runtime|런타임 시작)'){$phase='starting-runtime'}
                if($phase){$busy=$phase;if(-not $statusTexts.Contains($c.Name)){$statusTexts.Add((Short $c.Name))}}
            }
            if($c.ControlType -eq [Windows.Automation.ControlType]::Button -and $c.Name -in @('Run Locally','Run locally','로컬에서 실행')){$projectReady=$true}
            if($c.ControlType -eq [Windows.Automation.ControlType]::Button -and $c.Name -in @('Stop','중지')){
                $ancestor=$node
                for($i=0;$i -lt 48 -and $ancestor.parent;$i++){
                    $ancestor=$parents[$ancestor.parent];if($null -eq $ancestor){break}
                    if($ancestor.e.Current.ControlType -eq [Windows.Automation.ControlType]::TabItem -and $ancestor.e.Current.Name -in @('Console','콘솔')){$running=$true;break}
                }
            }
        }
    }
    if($projectReady -and $projectOpen){$state='project-ready'};if($running){$state='running'};if($busy){$state=$busy}
    if($dialogs.Count -gt 0){$state='modal'}
    if($scan.truncated -and $state -eq 'project-ready'){$state='unknown'}
    return @{state=$state;dialogs=$dialogs.ToArray();statusTexts=$statusTexts.ToArray()}
}
function Tree {
    $p=Target;$scan=Scan;$nodes=New-Object Collections.Generic.List[object]
    foreach($node in $scan.nodes){
        Check-Deadline
        $e=$node.e;$c=$e.Current;$bounds=$c.BoundingRectangle
        $publicBounds=@(0,0,0,0);if(-not $bounds.IsEmpty){$publicBounds=@($bounds.X,$bounds.Y,$bounds.Width,$bounds.Height)}
        $patterns=@($e.GetSupportedPatterns()|ForEach-Object{$_.ProgrammaticName.Replace('PatternIdentifiers.Pattern','')})
        $value=$null;$readOnly=$null
        if(-not $c.IsPassword -and $patterns -contains 'Value'){
            $v=$e.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current;$value=Short $v.Value;$readOnly=$v.IsReadOnly
        }
        $nodes.Add(@{elementId=$node.id;parentId=$node.parent;depth=$node.depth;role=$c.ControlType.ProgrammaticName.Replace('ControlType.','');name=$(if($c.IsPassword){'[password-redacted]'}else{Short $c.Name});automationId=$(if($c.IsPassword){''}else{Short $c.AutomationId});framework=$c.FrameworkId;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen;password=$c.IsPassword;focused=$c.HasKeyboardFocus;value=$value;readOnly=$readOnly;patterns=$patterns;bounds=$publicBounds})
    }
    $windows=New-Object Collections.Generic.List[object]
    foreach($h in @(Windows)){
        $root=[Windows.Automation.AutomationElement]::FromHandle($h)
        $windows.Add(@{elementId=(Register $root);handle=$h.ToInt64().ToString();name=(Short $root.Current.Name);modal=(Modal $root);minimized=[MendimaruUiNative]::IsIconic($h);foreground=([MendimaruUiNative]::GetForegroundWindow() -eq $h);dpi=[MendimaruUiNative]::GetDpiForWindow($h)})
    }
    $state=State $scan;$script:Revision++
    return @{sessionId=$script:Identity.sessionId;revision=$script:Revision;root=@{schemaVersion='5.0.0';hostPlatform=$script:Identity.hostPlatform;studioPlatform='windows';adapter=$script:Identity.fileVersion;generation=$script:Generation;processId=$p.Id;startedTicks=$script:Identity.startedTicks;interactiveSessionId=$p.SessionId;helperProcessId=$PID;helperSessionId=(Get-Process -Id $PID).SessionId;foregroundWindow=[MendimaruUiNative]::GetForegroundWindow().ToInt64().ToString();state=$state.state;statusTexts=$state.statusTexts;dialogs=$state.dialogs;windows=$windows.ToArray();nodes=$nodes.ToArray();truncated=$scan.truncated;limits=@{nodes=3000;depth=48;windows=16};fallbacks=@{coordinates=$false;keyboard=@('Tab','F5','Ctrl+G','Ctrl+S','Enter','Right','Escape');setValue='Properties Name only'}}}
}
function Input-Guard($e,[bool]$needsForeground) {
    $null=Target;Supported-Version
    if($e.Current.IsPassword -or $e.Current.IsOffscreen -or -not $e.Current.IsEnabled){throw 'ui-unsupported-element'}
    $h=Window-For $e
    if([MendimaruUiNative]::IsIconic($h) -or -not [MendimaruUiNative]::IsWindowEnabled($h)){throw 'ui-modal-blocked'}
    foreach($other in @(Windows)){
        $w=[Windows.Automation.AutomationElement]::FromHandle($other)
        # A nested dialog disables its modal owner. That owner must not block
        # input to the active child (WPF may also report disabled tool windows
        # as modal). Disabled targets are rejected above; enabled sibling
        # modals still prevent input from crossing the active dialog boundary.
        if($other -ne $h -and [MendimaruUiNative]::IsWindowEnabled($other) -and (Modal $w)){throw 'ui-modal-blocked'}
    }
    if(-not $needsForeground){return $h}
    $null=[MendimaruUiNative]::SetForegroundWindow($h)
    $focusDeadline=[DateTime]::UtcNow.AddMilliseconds(750)
    while([MendimaruUiNative]::GetForegroundWindow() -ne $h){
        Check-Deadline
        if([DateTime]::UtcNow -ge $focusDeadline){throw 'ui-foreground-lost'}
        Start-Sleep -Milliseconds 20
    }
    return $h
}
function Name-Editor($e) {
    if($e.Current.ControlType -ne [Windows.Automation.ControlType]::Edit){return $false}
    $row=$walker.GetParent($e);$property=$false;$panel=$false
    for($i=0;$i -lt 16 -and $null -ne $row;$i++){
        if($row.Current.Name -in @('Properties','속성','특성') -and $row.Current.ControlType -eq [Windows.Automation.ControlType]::TabItem){$panel=$true;break}
        if($i -lt 4){
            $child=$walker.GetFirstChild($row);$n=0
            while($null -ne $child -and $n++ -lt 32){
                if($child.Current.Name -in @('Name','이름') -and $child.Current.ControlType -eq [Windows.Automation.ControlType]::Text){$property=$true}
                $child=$walker.GetNextSibling($child)
            }
        }
        $row=$walker.GetParent($row)
    }
    return $panel -and $property
}
function Act($r) {
    Supported-Version
    $e=Resolve $r.elementId;$h=Input-Guard $e ($r.action -in @('focus','keyboard-input'));$pattern=$null
    $dispatchElement=Public-Element $e
    switch -CaseSensitive ($r.action) {
        'focus' {
            $e.SetFocus();if(-not $e.Current.HasKeyboardFocus){throw 'ui-effect-unverified'}
        }
        'invoke' {
            if(-not $e.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)){throw 'ui-unsupported-element'}
            $pattern.Invoke()
            # Invoke completion means UIA dispatched the semantic action. The
            # caller must wait for the intended application state separately.
        }
        'set-value' {
            if(-not (Name-Editor $e) -or $r.value -cnotmatch '^[A-Za-z_][A-Za-z0-9_]{0,99}$'){throw 'ui-unsupported-element'}
            if(-not $e.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern,[ref]$pattern) -or $pattern.Current.IsReadOnly){throw 'ui-unsupported-element'}
            $pattern.SetValue([string]$r.value)
            if($pattern.Current.Value -cne $r.value){throw 'ui-effect-unverified'}
        }
        'click' {
            # A semantic click selects a UIA SelectionItem and verifies its
            # selection. Canvas/image coordinate clicking is not enabled.
            if($e.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern,[ref]$pattern)){
                $pattern.Select();if(-not $pattern.Current.IsSelected){throw 'ui-effect-unverified'}
            }elseif($e.TryGetCurrentPattern([Windows.Automation.TogglePattern]::Pattern,[ref]$pattern)){
                $before=$pattern.Current.ToggleState;$pattern.Toggle()
                if($pattern.Current.ToggleState -eq $before){throw 'ui-effect-unverified'}
            }else{throw 'ui-unsupported-element'}
        }
        'keyboard-input' {
            if($e.Current.FrameworkId -cnotin @('WPF','WinForm','Win32')){throw 'ui-unsupported-element'}
            $keys=@{'Tab'='{TAB}';'F5'='{F5}';'Ctrl+G'='^g';'Ctrl+S'='^s';'Enter'='{ENTER}';'Right'='{RIGHT}';'Escape'='{ESC}'}
            if(-not $keys.ContainsKey([string]$r.value)){throw 'ui-unsupported-element'}
            if($r.value -eq 'F5'){
                if((State (Scan)).state -cne 'project-ready'){throw 'ui-effect-unverified'}
            }
            $e.SetFocus()
            if(-not $e.Current.HasKeyboardFocus -or [MendimaruUiNative]::GetForegroundWindow() -ne $h){throw 'ui-foreground-lost'}
            $null=Target
            [Windows.Forms.SendKeys]::SendWait($keys[[string]$r.value])
        }
        default {throw 'ui-invalid-request'}
    }
    $null=Target
    if($r.action -eq 'invoke'){return $dispatchElement}
    return Public-Element $e
}
function Capture($r) {
    $p=Target;$h=$p.MainWindowHandle
    if($r.windowId){$e=Resolve $r.windowId;$h=[IntPtr]$e.Current.NativeWindowHandle;if(@(Windows) -notcontains $h){throw 'ui-stale-element'}}
    $e=[Windows.Automation.AutomationElement]::FromHandle($h);$b=$e.Current.BoundingRectangle
    if([MendimaruUiNative]::IsIconic($h) -or $b.Width -le 0 -or $b.Height -le 0 -or $b.Width -gt 4096 -or $b.Height -gt 4096 -or $b.Width*$b.Height -gt 8388608){throw 'ui-capture-failed'}
    $bitmap=New-Object Drawing.Bitmap([int]$b.Width,[int]$b.Height)
    try {
        $g=[Drawing.Graphics]::FromImage($bitmap);$dc=$g.GetHdc()
        try {if(-not [MendimaruUiNative]::PrintWindow($h,$dc,2)){throw 'ui-capture-failed'}}
        finally {$g.ReleaseHdc($dc);$g.Dispose()}
        $image=$bitmap
        if($r.region){
            $a=$r.region
            if($a.Count -ne 4 -or $a[2] -le 0 -or $a[3] -le 0 -or $a[0]+$a[2] -gt $bitmap.Width -or $a[1]+$a[3] -gt $bitmap.Height){throw 'ui-invalid-request'}
            $rect=New-Object Drawing.Rectangle([int]$a[0],[int]$a[1],[int]$a[2],[int]$a[3]);$image=$bitmap.Clone($rect,$bitmap.PixelFormat)
        }
        $memory=New-Object IO.MemoryStream
        try {$image.Save($memory,[Drawing.Imaging.ImageFormat]::Png);if($memory.Length -gt 8388608){throw 'ui-capture-failed'};return @{png=[Convert]::ToBase64String($memory.ToArray())}}
        finally {$memory.Dispose();if($image -ne $bitmap){$image.Dispose()}}
    } finally {$bitmap.Dispose()}
}
function Execute($r) {
    if($r.sessionId -cne $script:Identity.sessionId){throw 'ui-wrong-session'}
    $null=Target
    Supported-Version
    switch -CaseSensitive ($r.operation) {
        'capabilities' {return @{sessionId=$r.sessionId;actions=@('invoke','click','focus','set-value','keyboard-input');adapter=$script:Identity.fileVersion;hostPlatform=$script:Identity.hostPlatform;studioPlatform='windows';coordinates=$false;setValue='Properties Name only';keys=@('Tab','F5','Ctrl+G','Ctrl+S','Enter','Right','Escape');conditions=@('project-ready','building','deploying','starting-runtime','running','modal')}}
        'tree' {return Tree}
        'find' {return ,@(Find-Elements $r.selector | ForEach-Object { foreach($e in $_){Public-Element $e} })}
        'action' {return Act $r}
        'screenshot' {return Capture $r}
        'wait' {
            $waitScope=$null;if($r.selector.scopeId){$waitScope=Resolve $r.selector.scopeId}
            while($true){
                $null=Target
                if($r.selector){
                    $found=@(Find-Elements $r.selector $waitScope)
                    if($found.Count -gt 1){throw 'ui-ambiguous-element'}
                    if($found.Count -eq 1 -and $found[0].Current.IsEnabled -and -not $found[0].Current.IsOffscreen){return Public-Element $found[0]}
                } else {
                    $scan=Scan
                    if($scan.truncated){throw 'ui-tree-truncated'}
                    if((State $scan).state -ceq $r.condition){return Public-Element (Root)}
                }
                Start-Sleep -Milliseconds 100
            }
        }
        default {throw 'ui-invalid-request'}
    }
}
while($null -ne ($line=[Console]::ReadLine())) {
    try {
        if($line.Length -gt 16384){throw 'ui-invalid-request'}
        $message=$line|ConvertFrom-Json
        $r=$message.request
        $script:Deadline=[DateTimeOffset]::FromUnixTimeMilliseconds([long]$message.expiresAt).UtcDateTime
        if($r.timeoutMs -lt 100 -or $r.timeoutMs -gt 60000){throw 'ui-invalid-request'}
        $data=Execute $r
        $result=@{ok=$true;data=$data}
    } catch {
        $reason=$_.Exception.Message
        $baseException=$_.Exception.GetBaseException()
        if($baseException -is [Windows.Automation.ElementNotAvailableException]){$reason='ui-stale-element'}
        elseif($baseException -is [InvalidOperationException] -or $baseException -is [NotSupportedException]){$reason='ui-unsupported-element'}
        if($reason -cnotmatch '^ui-[a-z-]+$'){$reason='ui-provider-failed'}
        $result=@{ok=$false;reason=$reason;diagnostic=@{exceptionType=$baseException.GetType().FullName;hresult=$baseException.HResult}}
    }
    $serialized=ConvertTo-Json -InputObject $result -Depth 16 -Compress
    if($serialized.Length -gt 12582912){$serialized='{"ok":false,"reason":"ui-tree-truncated"}'}
    [Console]::WriteLine($serialized)
}
