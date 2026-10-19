$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$source = [IO.File]::ReadAllText((Join-Path $root 'src-tauri/scripts/studio_m2ee.cs'))
$source += "`n" + [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'fixtures/studio-m2ee-tests.cs'))
Add-Type -TypeDefinition $source -Language CSharp -ErrorAction Stop
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    Write-Output 'PASS: admin bridge and ownership fixture compile; Windows process tests require Windows'
    exit 0
}
$temp = Join-Path ([IO.Path]::GetTempPath()) ('mendimaru-m2ee-test-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $temp
$studio = $null
$foreign = $null
$controlName = 'Local\mendimaru-m2ee-' + [Guid]::NewGuid().ToString('N')
$startSignal = New-Object Threading.EventWaitHandle($false, [Threading.EventResetMode]::AutoReset, ($controlName + '-start'))
$stopSignal = New-Object Threading.EventWaitHandle($false, [Threading.EventResetMode]::AutoReset, ($controlName + '-stop'))
$exitSignal = New-Object Threading.EventWaitHandle($false, [Threading.EventResetMode]::AutoReset, ($controlName + '-exit'))
function Read-FixtureLine($process, [string]$stage) {
    $pending = $process.StandardOutput.ReadLineAsync()
    if (-not $pending.Wait(10000)) { throw "Fixture timed out: $stage" }
    return $pending.GetAwaiter().GetResult()
}
try {
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    $executable = Join-Path $temp 'StudioPro.exe'
    & $compiler /nologo /codepage:65001 /target:exe "/out:$executable" (Join-Path $PSScriptRoot 'fixtures/studio-m2ee.cs')
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile isolated Studio process fixture' }
    Copy-Item -LiteralPath $executable -Destination (Join-Path $temp 'java.exe')
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $executable
    $start.Arguments = $controlName
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $studio = [Diagnostics.Process]::Start($start)
    if ((Read-FixtureLine $studio 'Studio ready') -cne 'ready') { throw 'Studio fixture did not start' }
    Write-Output 'Fixture: Studio ready'
    $alias = 'test-only-é-한글-' + [Guid]::NewGuid().ToString('N')
    $ticks = $studio.StartTime.ToUniversalTime().Ticks
    foreach ($round in @(1, 2)) {
        $null = $startSignal.Set()
        if ((Read-FixtureLine $studio 'Runtime started') -cne 'started') { throw 'Runtime fixture did not start' }
        Write-Output "Fixture: owned Runtime round $round ready"
        [MendimaruRuntime.BridgeTests]::Verify($studio.Id, $ticks, $executable, $alias)
        $null = $stopSignal.Set()
        if ((Read-FixtureLine $studio 'Runtime stopped') -cne 'stopped') { throw 'Runtime fixture did not stop' }
    }
    $start.FileName = Join-Path $temp 'java.exe'
    $start.Arguments = 'java'
    $start.EnvironmentVariables['M2EE_ADMIN_PASS'] = [Guid]::NewGuid().ToString('N')
    $foreign = [Diagnostics.Process]::Start($start)
    if ((Read-FixtureLine $foreign 'Foreign Runtime ready') -cne 'ready') { throw 'Foreign Runtime fixture did not start' }
    [MendimaruRuntime.BridgeTests]::RefuseForeignListener($studio.Id, $ticks, $executable, $alias)
    Write-Output 'PASS: owned Runtime environment, UTF-8 alias, missing/wrong/valid servlet and M2EE auth, shutdown, restart, unsafe HTTP, stale Studio and foreign listener refusal'
} finally {
    $null = $exitSignal.Set()
    if ($null -ne $studio) { try { $null = $studio.WaitForExit(3000) } catch {} }
    foreach ($process in @($foreign, $studio)) {
        if ($null -ne $process) {
            try {
                if (-not $process.HasExited) {
                    $process.Kill()
                }
                $null = $process.WaitForExit(5000)
            } catch {}
            $process.Dispose()
        }
    }
    foreach ($signal in @($startSignal, $stopSignal, $exitSignal)) { $signal.Dispose() }
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
