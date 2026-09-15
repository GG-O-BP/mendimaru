param(
    [Parameter(Mandatory = $true)][int]$StudioProcessId,
    [Parameter(Mandatory = $true)][string]$StartTimeUtcTicks,
    [Parameter(Mandatory = $true)][string]$FileVersion,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$reason = 'initialization-failed'
try {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase, System.Drawing, System.Windows.Forms
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class StudioUiaCapture {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr parameter);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr parameter);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] public static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr handle, int index, StringBuilder value, uint length, out uint needed);
    public static bool IsDefaultInputDesktop() {
        var desktop = OpenInputDesktop(0, false, 1);
        if (desktop == IntPtr.Zero) return false;
        try {
            var name = new StringBuilder(256); uint needed;
            return GetUserObjectInformation(desktop, 2, name, 512, out needed) && name.ToString() == "Default";
        } finally { CloseDesktop(desktop); }
    }
    public static IntPtr[] Windows(uint pid) {
        var result = new List<IntPtr>();
        EnumWindows((hwnd, parameter) => {
            uint owner; GetWindowThreadProcessId(hwnd, out owner);
            if (owner == pid && IsWindowVisible(hwnd)) result.Add(hwnd);
            // The ninth window is a sentinel: never silently report eight as
            // a complete top-level inventory when more windows are present.
            return result.Count < 9;
        }, IntPtr.Zero);
        return result.ToArray();
    }
}
'@
    $null = [StudioUiaCapture]::SetProcessDpiAwarenessContext([IntPtr](-4))
    $reason = 'stale-or-wrong-process'
    function Get-Target {
        $target = Get-Process -Id $StudioProcessId -ErrorAction Stop
        if ($target.ProcessName -ne 'studiopro' -or
            $target.StartTime.ToUniversalTime().Ticks.ToString() -cne $StartTimeUtcTicks -or
            $target.MainModule.FileVersionInfo.FileVersion -cne $FileVersion) { throw 'identity-mismatch' }
        $current = Get-Process -Id $PID
        if ($target.SessionId -eq 0 -or $target.SessionId -ne $current.SessionId) { throw 'wrong-session' }
        return $target
    }
    $target = Get-Target
    $reason = 'no-interactive-desktop'
    if (-not [StudioUiaCapture]::IsDefaultInputDesktop()) { throw $reason }
    $reason = 'no-visible-window'
    $windows = @([StudioUiaCapture]::Windows([uint32]$StudioProcessId))
    if ($windows.Count -eq 0) { throw $reason }
    $windowsTruncated = $windows.Count -gt 8
    $snapshots = New-Object Collections.Generic.List[object]
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $walker = [Windows.Automation.TreeWalker]::RawViewWalker
    foreach ($handle in @($windows | Select-Object -First 8)) {
        $null = Get-Target
        $reason = 'provider-failed'
        $root = [Windows.Automation.AutomationElement]::FromHandle($handle)
        $queue = New-Object Collections.Generic.Queue[object]
        $queue.Enqueue(@{ element = $root; parent = $null; depth = 0 })
        $nodes = New-Object Collections.Generic.List[object]
        $truncated = $false
        while ($queue.Count -gt 0) {
            if ($nodes.Count -ge 2000 -or $clock.ElapsedMilliseconds -ge 12000) { $truncated = $true; break }
            $item = $queue.Dequeue()
            $element = $item.element
            $current = $element.Current
            $name = $current.Name
            if ($current.IsPassword) { $name = '[password-redacted]' }
            if ($name.Length -gt 256) { $name = $name.Substring(0, 256) }
            $automationId = $current.AutomationId
            if ($automationId.Length -gt 256) { $automationId = $automationId.Substring(0, 256) }
            $rect = $current.BoundingRectangle
            $nodeId = $nodes.Count
            $state = [ordered]@{}
            $supported = @($element.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName })
            if (-not $current.IsPassword -and $supported -contains 'ValuePatternIdentifiers.Pattern') {
                $value = ([Windows.Automation.ValuePattern]$element.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).Current
                $state.readOnly = $value.IsReadOnly
                $text = $value.Value
                $state.valueTruncated = $text.Length -gt 256
                $state.value = $text.Substring(0, [Math]::Min($text.Length, 256))
            }
            if ($supported -contains 'SelectionItemPatternIdentifiers.Pattern') {
                $state.selected = ([Windows.Automation.SelectionItemPattern]$element.GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern)).Current.IsSelected
            }
            if ($supported -contains 'TogglePatternIdentifiers.Pattern') {
                $state.toggle = ([Windows.Automation.TogglePattern]$element.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern)).Current.ToggleState.ToString()
            }
            if ($supported -contains 'ExpandCollapsePatternIdentifiers.Pattern') {
                $state.expansion = ([Windows.Automation.ExpandCollapsePattern]$element.GetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern)).Current.ExpandCollapseState.ToString()
            }
            $nodes.Add([ordered]@{
                id = $nodeId; parent = $item.parent; depth = $item.depth
                name = $name; automationId = $automationId; controlType = $current.ControlType.ProgrammaticName
                className = $current.ClassName; frameworkId = $current.FrameworkId
                processId = $current.ProcessId; enabled = $current.IsEnabled; offscreen = $current.IsOffscreen
                password = $current.IsPassword; keyboardFocusable = $current.IsKeyboardFocusable
                keyboardFocused = $current.HasKeyboardFocus; state = $state
                bounds = @($rect.X, $rect.Y, $rect.Width, $rect.Height)
                patterns = $supported
            })
            $child = $walker.GetFirstChild($element)
            if ($item.depth -ge 18) {
                if ($null -ne $child) { $truncated = $true }
                continue
            }
            while ($null -ne $child) {
                if ($queue.Count + $nodes.Count -ge 2000 -or $clock.ElapsedMilliseconds -ge 12000) { $truncated = $true; break }
                $queue.Enqueue(@{ element = $child; parent = $nodeId; depth = $item.depth + 1 })
                $child = $walker.GetNextSibling($child)
            }
        }
        $pattern = $null
        $modal = $null
        if ($root.TryGetCurrentPattern([Windows.Automation.WindowPattern]::Pattern, [ref]$pattern)) {
            $modal = ([Windows.Automation.WindowPattern]$pattern).Current.IsModal
        }
        $bounds = $root.Current.BoundingRectangle
        $screenshot = @{ status = 'unavailable'; reason = 'invalid-or-minimized-bounds' }
        if (-not [StudioUiaCapture]::IsIconic($handle) -and $bounds.Width -gt 0 -and $bounds.Height -gt 0 -and
            $bounds.Width -le 4096 -and $bounds.Height -le 4096 -and $bounds.Width * $bounds.Height -le 8388608) {
            $bitmap = New-Object Drawing.Bitmap([int]$bounds.Width, [int]$bounds.Height)
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            $dc = $graphics.GetHdc()
            try { $accepted = [StudioUiaCapture]::PrintWindow($handle, $dc, 2) }
            finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
            try {
                if ($accepted) {
                    $filename = 'window-' + $snapshots.Count + '.png'
                    $bitmap.Save((Join-Path $OutputDirectory $filename), [Drawing.Imaging.ImageFormat]::Png)
                    $screenshot = @{ status = 'captured'; method = 'PrintWindow'; file = $filename; visualVerification = 'required' }
                } else { $screenshot.reason = 'printwindow-rejected' }
            } finally { $bitmap.Dispose() }
        }
        $snapshots.Add([ordered]@{
            hwnd = $handle.ToInt64().ToString(); dpi = [StudioUiaCapture]::GetDpiForWindow($handle)
            modal = $modal; truncated = $truncated; nodes = @($nodes.ToArray()); screenshot = $screenshot
        })
    }
    $null = Get-Target
    $reason = 'desktop-changed-during-capture'
    if (-not [StudioUiaCapture]::IsDefaultInputDesktop()) { throw $reason }
    $report = [ordered]@{
        schemaVersion = 1; status = 'captured'; provider = 'dotnet-framework-uia'
        collectedAtUtc = [DateTime]::UtcNow.ToString('o'); elapsedMs = $clock.ElapsedMilliseconds
        studioProcessId = $StudioProcessId; startTimeUtcTicks = $StartTimeUtcTicks
        fileVersion = $FileVersion; sessionId = $target.SessionId; helperSessionId = (Get-Process -Id $PID).SessionId
        culture = [Globalization.CultureInfo]::CurrentUICulture.Name
        powershellVersion = $PSVersionTable.PSVersion.ToString()
        windowsTruncated = $windowsTruncated
        screenBounds = @([Windows.Forms.Screen]::AllScreens | ForEach-Object {
            @{ width = $_.Bounds.Width; height = $_.Bounds.Height; primary = $_.Primary }
        })
        windows = @($snapshots.ToArray())
    }
    $report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'snapshot.json') -Encoding UTF8
    exit 0
} catch {
    # Do not retain exception messages: they can contain private model names,
    # paths or account information. Full snapshots are private evidence too.
    @{ schemaVersion = 1; status = 'fail'; reason = $reason } | ConvertTo-Json |
        Set-Content -LiteralPath (Join-Path $OutputDirectory 'worker-failure.json') -Encoding UTF8
    exit 1
}
