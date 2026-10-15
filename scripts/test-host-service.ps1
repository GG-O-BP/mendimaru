$ErrorActionPreference = 'Stop'
# .NET Framework helper의 실제 공급 소스를 컴파일하고 소켓 소유권을 검증한다.
$source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../src-tauri/scripts/host_service.cs'))
Add-Type -TypeDefinition $source -Language CSharp -ErrorAction Stop
$key = New-Object byte[] 32
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
$rng.GetBytes($key)
$rng.Dispose()
$first = $null
$second = $null
$independent = $null
$fixtureListener = $null
$connections = New-Object 'System.Collections.Generic.List[System.Net.Sockets.TcpClient]'
try {
    $first = New-Object MendimaruHostService('127.0.0.1', 9, ('0' * 64), 0, $key)
    $port = $first.GuestPort
    if ($port -lt 1) { throw 'No guest port allocated' }
    $collision = $false
    try { $second = New-Object MendimaruHostService('127.0.0.1', 9, ('0' * 64), $port, $key) }
    catch { $collision = $true }
    if (-not $collision) { throw 'Guest port ownership was not exclusive' }
    $independent = New-Object MendimaruHostService('127.0.0.1', 9, ('0' * 64), 0, $key)
    $independentPort = $independent.GuestPort
    if ($independentPort -eq $port) { throw 'Independent owners shared a listener' }
    $first.Dispose()
    if (-not $first.IsClosed) { throw 'Disposed helper was not observable as closed' }
    $first = $null
    $second = New-Object MendimaruHostService('127.0.0.1', 9, ('0' * 64), $port, $key)
    if ($second.GuestPort -ne $port) { throw 'Owned port could not be reused after disposal' }
    if ($independent.IsClosed -or $independent.GuestPort -ne $independentPort) { throw 'Disposal affected another owner' }
    $field = $second.GetType().GetField('listener', [Reflection.BindingFlags]'Instance,NonPublic')
    $listener = $field.GetValue($second)
    if (-not $listener.LocalEndpoint.Address.Equals([Net.IPAddress]::Loopback)) { throw 'Guest listener escaped loopback' }
    $fixtureListener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    $fixtureListener.Start()
    $fixturePort = ([Net.IPEndPoint]$fixtureListener.LocalEndpoint).Port
    $drain = [MendimaruHostService].GetMethod('Drain', [Reflection.BindingFlags]'Static,NonPublic')
    foreach ($expires in @($false, $true)) {
        $client = New-Object Net.Sockets.TcpClient
        $client.Connect([Net.IPAddress]::Loopback, $fixturePort)
        $clientPeer = $fixtureListener.AcceptTcpClient()
        $upstream = New-Object Net.Sockets.TcpClient
        $upstream.Connect([Net.IPAddress]::Loopback, $fixturePort)
        $upstreamPeer = $fixtureListener.AcceptTcpClient()
        foreach ($connection in @($client, $clientPeer, $upstream, $upstreamPeer)) { $connections.Add($connection) }
        $bytes = New-Object byte[] 1
        $pending = $client.GetStream().ReadAsync($bytes, 0, 1)
        $completed = [Threading.Tasks.Task]::Delay(0)
        if (-not $expires) { $clientPeer.GetStream().WriteByte(42) }
        $grace = if ($expires) { 100 } else { 5000 }
        # PS 5.1은 object[]에 New-Object의 PSObject wrapper를 남긴다.
        # reflection에는 실제 .NET 객체를 전달해야 소켓 정리 경로를 실행한다.
        $task = $drain.Invoke($null, [object[]]@($completed.PSObject.BaseObject, $pending.PSObject.BaseObject, $client.PSObject.BaseObject, $upstream.PSObject.BaseObject, [int]$grace))
        $failed = $false
        try { $null = $task.GetAwaiter().GetResult() } catch { $failed = $true }
        if ($expires) {
            if (-not $pending.IsCompleted) { throw 'Stalled half-close did not release the pending stream' }
            foreach ($peer in @($clientPeer, $upstreamPeer)) {
                $peer.GetStream().ReadTimeout = 1000
                if ($peer.GetStream().ReadByte() -ne -1) { throw 'Half-close expiry leaked an owned socket' }
            }
        } else {
            if ($failed -or $bytes[0] -ne 42) { throw 'Normal half-close lost the opposite response' }
            $client.GetStream().WriteByte(43)
            $clientPeer.GetStream().ReadTimeout = 1000
            if ($clientPeer.GetStream().ReadByte() -ne 43) { throw 'Normal drain closed a live opposite direction' }
        }
        if ($second.IsClosed -or $independent.IsClosed) { throw 'Stream cleanup closed an unrelated listener' }
    }
    Write-Output 'PASS: production helper compiles, loopback ownership, independent listeners, port reuse, half-close response and bounded cleanup'
} finally {
    if ($null -ne $first) { $first.Dispose() }
    if ($null -ne $second) { $second.Dispose() }
    if ($null -ne $independent) { $independent.Dispose() }
    if ($null -ne $fixtureListener) { $fixtureListener.Stop() }
    foreach ($connection in $connections) { $connection.Dispose() }
    [Array]::Clear($key, 0, $key.Length)
}
