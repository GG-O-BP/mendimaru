# Exercise production locators/actions against test-owned native WPF dialogs.
# Only Studio process identity is substituted; UIA, ownership and modal checks
# use real windows. This is not a substitute for live Studio acceptance.
$ErrorActionPreference='Stop'
$text=[IO.File]::ReadAllText((Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\scripts\ui_worker.ps1'))
. ([ScriptBlock]::Create($text.Substring(0,$text.IndexOf('$null = [MendimaruUiNative]::SetThreadDpiAwarenessContext'))))
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseInput($text,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'worker syntax errors'}
foreach($definition in $ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]},$true)){
    . ([ScriptBlock]::Create($definition.Extent.Text))
}
$walker=[Windows.Automation.TreeWalker]::RawViewWalker
$script:ObservationCache=New-ObservationCache
$lab=Join-Path $env:TEMP ('mendimaru-navigation-test-'+[Guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $lab
$fixture=Join-Path $lab 'fixture.ps1'
[IO.File]::WriteAllText($fixture,@'
param($StatePath,$Mode)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName PresentationFramework
$main=New-Object Windows.Window
$main.Title='Mendimaru navigation test owner';$main.Width=400;$main.Height=300
$dialog=New-Object Windows.Window
$dialog.Title=$(if($Mode -eq 'unrelated'){'Other dialog'}else{'Go To'})
$dialog.Width=300;$dialog.Height=200
$panel=New-Object Windows.Controls.StackPanel
$edit=New-Object Windows.Controls.TextBox
$edit.Text='Original';$edit.Name='SearchEditor';$panel.Children.Add($edit)|Out-Null
if($Mode -eq 'ambiguous'){$panel.Children.Add((New-Object Windows.Controls.TextBox))|Out-Null}
$dialog.Content=$panel
$timer=New-Object Windows.Threading.DispatcherTimer
$timer.Interval=[TimeSpan]::FromMilliseconds(50)
$timer.Add_Tick({
    $timer.Stop()
    [IO.File]::WriteAllText($StatePath,(@{dialog=(New-Object Windows.Interop.WindowInteropHelper($dialog)).Handle.ToInt64()}|ConvertTo-Json -Compress))
})
$dialog.Add_ContentRendered({$timer.Start()})
$main.Show();$dialog.Owner=$main;$null=$dialog.ShowDialog()
'@)
try {
    foreach($mode in @('unique','ambiguous','unrelated')){
        $state=Join-Path $lab ($mode+'.json');$process=$null
        try {
            $process=Start-Process (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile','-STA','-File',('"'+$fixture+'"'),('"'+$state+'"'),$mode) -PassThru
            $until=[DateTime]::UtcNow.AddSeconds(20)
            while(-not (Test-Path $state)){
                if($process.HasExited -or [DateTime]::UtcNow -ge $until){throw 'navigation fixture did not start'}
                Start-Sleep -Milliseconds 50
            }
            $script:Deadline=[DateTime]::UtcNow.AddSeconds(30)
            function Target { Check-Deadline;return $process }
            $script:Identity=@{fileVersion='10.24.26.0'}
            $script:Generation=[Guid]::NewGuid().ToString('N');$script:Elements=@{};$script:ElementSequence=0
            $handles=[IO.File]::ReadAllText($state)|ConvertFrom-Json
            $dialog=[Windows.Automation.AutomationElement]::FromHandle([IntPtr]$handles.dialog)
            $condition=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::AutomationIdProperty,'SearchEditor')
            $edit=$dialog.FindFirst([Windows.Automation.TreeScope]::Descendants,$condition)
            if($null -eq $edit){throw 'search editor missing'}
            $scan=Scan $dialog
            if($scan.truncated){throw 'native dialog tree truncated'}
            $snapshot=@($scan.nodes|Where-Object{$_.observed.c.AutomationId -ceq 'SearchEditor'})
            if($snapshot.Count -ne 1 -or $snapshot[0].observed.value -cne 'Original'){throw 'cached editor observation missing'}
            $id=$snapshot[0].id
            $request=@{elementId=$id;action='set-value';value='Home_Web'}
            if($mode -eq 'unique'){
                if(-not (GoTo-Editor $edit)){throw 'known unique Go To editor rejected'}
                $null=Act $request
                $value=$edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value
                if($value -cne 'Home_Web'){throw 'Go To value readback failed'}
                $rejected=$false
                try{$null=Act $request}catch{$rejected=$_.Exception.Message -ceq 'ui-stale-element'}
                if(-not $rejected){throw 'old value handle remained valid'}
                $request.elementId=Register $edit;$request.value='../../arbitrary'
            }
            $rejected=$false
            try{$null=Act $request}catch{$rejected=$_.Exception.Message -ceq 'ui-unsupported-element'}
            if(-not $rejected){throw ('unsafe navigation write accepted: '+$mode)}
            Write-Output ('UI navigation: '+$mode+' passed.')
        } finally {
            if($process -and -not $process.HasExited){$process.Kill();$null=$process.WaitForExit(5000)}
            if($process){$process.Dispose()}
        }
    }
} finally {Remove-Item -LiteralPath $lab -Recurse -Force -ErrorAction SilentlyContinue}
