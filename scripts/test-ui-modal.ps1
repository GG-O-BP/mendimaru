# Production input guard against nested native modal windows. Only the Studio
# identity lookup is substituted; ownership traversal, Win32 enabled state and
# UIA modal state use real windows in a separate, test-owned process.
$ErrorActionPreference='Stop'
$source=Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\scripts\ui_worker.ps1'
$text=[IO.File]::ReadAllText($source)
. ([ScriptBlock]::Create($text.Substring(0,$text.IndexOf('$null = [MendimaruUiNative]::SetThreadDpiAwarenessContext'))))
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseInput($text,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'worker syntax errors'}
foreach($name in @('Check-Deadline','Modal','Window-For','Input-Guard')){
    $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name},$true)
    . ([ScriptBlock]::Create($definition.Extent.Text))
}
$walker=[Windows.Automation.TreeWalker]::RawViewWalker
$lab=Join-Path $env:TEMP ('mendimaru-modal-test-'+[Guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $lab
$fixture=Join-Path $lab 'fixture.ps1'
$state=Join-Path $lab 'state.json'
[IO.File]::WriteAllText($fixture,@'
param($StatePath)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
$main=New-Object Windows.Forms.Form
$main.Text='Mendimaru test main'
$owner=New-Object Windows.Forms.Form
$owner.Text='Mendimaru test modal owner'
$child=New-Object Windows.Forms.Form
$child.Text='Mendimaru test active modal'
$button=New-Object Windows.Forms.Button
$button.Text='Continue'
$child.Controls.Add($button)
$timer=New-Object Windows.Forms.Timer
$timer.Interval=50
$timer.Add_Tick({
    $timer.Stop()
    [IO.File]::WriteAllText($StatePath,(@{main=$main.Handle.ToInt64();owner=$owner.Handle.ToInt64();child=$child.Handle.ToInt64();button=$button.Handle.ToInt64()}|ConvertTo-Json -Compress))
})
$child.Add_Shown({$timer.Start()})
$owner.Add_Shown({$null=$child.ShowDialog($owner)})
$main.Show()
$null=$owner.ShowDialog($main)
'@)
$process=$null
try {
    $process=Start-Process (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile','-STA','-File',('"'+$fixture+'"'),('"'+$state+'"')) -PassThru
    $until=[DateTime]::UtcNow.AddSeconds(20)
    while(-not (Test-Path $state)){
        if($process.HasExited -or [DateTime]::UtcNow -ge $until){throw 'modal fixture did not start'}
        Start-Sleep -Milliseconds 50
    }
    $handles=[IO.File]::ReadAllText($state)|ConvertFrom-Json
    $script:Deadline=[DateTime]::UtcNow.AddSeconds(20)
    function Target { Check-Deadline;return $process }
    function Supported-Version {}
    function Windows { return @([IntPtr]$handles.main,[IntPtr]$handles.owner,[IntPtr]$handles.child) }
    $ownerElement=[Windows.Automation.AutomationElement]::FromHandle([IntPtr]$handles.owner)
    $childElement=[Windows.Automation.AutomationElement]::FromHandle([IntPtr]$handles.child)
    if(-not (Modal $ownerElement) -or -not (Modal $childElement)){throw 'fixture did not create nested UIA modals'}
    if([MendimaruUiNative]::IsWindowEnabled([IntPtr]$handles.owner)){throw 'modal owner must be disabled'}
    $button=[Windows.Automation.AutomationElement]::FromHandle([IntPtr]$handles.button)
    if((Input-Guard $button $false) -ne [IntPtr]$handles.child){throw 'active modal child was not permitted'}
    $rejected=$false
    try {$null=Input-Guard $ownerElement $false} catch {$rejected=$_.Exception.Message -in @('ui-unsupported-element','ui-modal-blocked')}
    if(-not $rejected){throw 'disabled modal owner accepted input'}
    Write-Output 'UI input guard: active nested dialog permitted; disabled owner rejected.'
} finally {
    if($process -and -not $process.HasExited){$process.Kill();$null=$process.WaitForExit(5000)}
    if($process){$process.Dispose()}
    Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue
}
