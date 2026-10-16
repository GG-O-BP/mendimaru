// Windows PowerShell 5.1의 .NET Framework에서 컴파일한다. DB 바이트는 공유
// 파일/PowerShell 파이프라인을 거치지 않고 비동기 소켓과 TLS로만 전송한다.
using System;
using System.IO;
using System.Net;
using System.Net.Security;
using System.Net.Sockets;
using System.Security.Authentication;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
using System.Threading;
using System.Threading.Tasks;

public sealed class MendimaruHostService : IDisposable {
    const int FrameBytes = 16384;
    readonly TcpListener listener;
    readonly string host, pin;
    readonly int port;
    readonly byte[] key;
    readonly CancellationTokenSource cancel = new CancellationTokenSource();
    readonly SemaphoreSlim capacity = new SemaphoreSlim(64, 64);
    public bool IsClosed { get { return cancel.IsCancellationRequested; } }
    public int GuestPort { get { return ((IPEndPoint)listener.LocalEndpoint).Port; } }

    public MendimaruHostService(string host, int port, string pin, int guestPort, byte[] operationKey) {
        this.host = host; this.port = port; this.pin = pin;
        using (var mac = new HMACSHA256(operationKey)) {
            key = mac.ComputeHash(System.Text.Encoding.ASCII.GetBytes("mendimaru-host-service-v1"));
        }
        listener = new TcpListener(IPAddress.Loopback, guestPort);
        listener.Server.ExclusiveAddressUse = true;
        listener.Start(64);
    }

    bool ValidateCertificate(object sender, X509Certificate cert, X509Chain chain, SslPolicyErrors errors) {
        if (cert == null) return false;
        using (var sha = SHA256.Create()) {
            string actual = BitConverter.ToString(sha.ComputeHash(cert.GetRawCertData())).Replace("-", "").ToLowerInvariant();
            return String.Equals(pin, actual, StringComparison.Ordinal);
        }
    }

    async Task Exact(Stream stream, byte[] bytes, int count) {
        int offset = 0;
        while (offset < count) {
            int got = await stream.ReadAsync(bytes, offset, count - offset, cancel.Token).ConfigureAwait(false);
            if (got == 0) throw new EndOfStreamException();
            offset += got;
        }
    }

    async Task<SslStream> Connect(TcpClient tcp, byte kind) {
        tcp.NoDelay = true;
        await tcp.ConnectAsync(host, port).ConfigureAwait(false);
        var ssl = new SslStream(tcp.GetStream(), false, ValidateCertificate);
        try {
            await ssl.AuthenticateAsClientAsync("mendimaru-host-service", null, SslProtocols.Tls12, false).ConfigureAwait(false);
            await ssl.WriteAsync(new byte[] { kind }, 0, 1, cancel.Token).ConfigureAwait(false);
            var challenge = new byte[33];
            await Exact(ssl, challenge, 32).ConfigureAwait(false);
            challenge[32] = kind;
            byte[] proof;
            using (var mac = new HMACSHA256(key)) { proof = mac.ComputeHash(challenge); }
            await ssl.WriteAsync(proof, 0, proof.Length, cancel.Token).ConfigureAwait(false);
            Array.Clear(proof, 0, proof.Length);
            return ssl;
        } catch { ssl.Dispose(); throw; }
    }

    async Task Encode(Stream source, Stream wire) {
        byte[] data = new byte[FrameBytes + 4];
        for (;;) {
            int count = await source.ReadAsync(data, 4, FrameBytes, cancel.Token).ConfigureAwait(false);
            // 헤더를 합쳐 매 프레임의 작은 추가 TLS 레코드와 할당을 없앤다.
            data[0] = (byte)(count >> 24); data[1] = (byte)(count >> 16); data[2] = (byte)(count >> 8); data[3] = (byte)count;
            await wire.WriteAsync(data, 0, count + 4, cancel.Token).ConfigureAwait(false);
            if (count == 0) return;
        }
    }

    async Task Decode(Stream wire, TcpClient client) {
        byte[] header = new byte[4], data = new byte[FrameBytes];
        for (;;) {
            await Exact(wire, header, 4).ConfigureAwait(false);
            uint count = ((uint)header[0] << 24) | ((uint)header[1] << 16) | ((uint)header[2] << 8) | header[3];
            if (count > FrameBytes) throw new InvalidDataException();
            if (count == 0) { client.Client.Shutdown(SocketShutdown.Send); return; }
            await Exact(wire, data, (int)count).ConfigureAwait(false);
            await client.GetStream().WriteAsync(data, 0, (int)count, cancel.Token).ConfigureAwait(false);
        }
    }

    static async Task Drain(Task send, Task receive, TcpClient client, TcpClient upstream, int graceMilliseconds) {
        Task first = await Task.WhenAny(send, receive).ConfigureAwait(false);
        if (first.IsFaulted || first.IsCanceled) { client.Close(); upstream.Close(); }
        else {
            // 한쪽 EOF 뒤에도 마지막 응답을 보존하되, 응답하지 않는 로컬 peer가
            // 64개 슬롯을 영구 점유하지 않게 half-close 완료 대기를 제한한다.
            Task remaining = first == send ? receive : send;
            using (var deadline = new CancellationTokenSource()) {
                if (await Task.WhenAny(remaining, Task.Delay(graceMilliseconds, deadline.Token)).ConfigureAwait(false) != remaining) {
                    client.Close(); upstream.Close();
                }
                deadline.Cancel();
            }
        }
        await Task.WhenAll(send, receive).ConfigureAwait(false);
    }

    async Task Serve(TcpClient client) {
        using (client)
        using (var upstream = new TcpClient())
        using (cancel.Token.Register(() => { client.Close(); upstream.Close(); })) {
            try {
                // 연결/TLS/인증에 10초를 넘겨 쓰지 않는다. 취소 시 실제 소켓도 닫는다.
                var opening = Connect(upstream, 1);
                if (await Task.WhenAny(opening, Task.Delay(10000, cancel.Token)).ConfigureAwait(false) != opening) {
                    upstream.Close();
                    try { using (await opening.ConfigureAwait(false)) {} } catch {}
                    return;
                }
                using (var ssl = await opening.ConfigureAwait(false)) {
                    byte[] ack = new byte[1];
                    await Exact(ssl, ack, 1).ConfigureAwait(false);
                    if (ack[0] != 1) return;
                    client.NoDelay = true;
                    Task send = Encode(client.GetStream(), ssl), receive = Decode(ssl, client);
                    // 방향별 유휴 만료는 호스트가, EOF 뒤 완료 기한은 게스트가 담당한다.
                    await Drain(send, receive, client, upstream, 300000).ConfigureAwait(false);
                }
            } catch {} finally { capacity.Release(); }
        }
    }

    async Task Accept() {
        while (!cancel.IsCancellationRequested) {
            TcpClient client = await listener.AcceptTcpClientAsync().ConfigureAwait(false);
            if (!capacity.Wait(0)) { client.Close(); continue; }
            // 최대 64개 작업만 존재한다. 완료된 작업을 목록에 보관하지 않는다.
            Task serving = Serve(client);
        }
    }

    // 게스트의 localhost 전용 HTTP를 외부 NIC에 노출하지 않는다. 인증된
    // outbound TLS 연결을 미리 열고, 호스트가 요청할 때만 고정 포트에 연결한다.
    async Task Reverse(byte kind, int targetPort) {
        while (!cancel.IsCancellationRequested) {
            try {
                using (var upstream = new TcpClient())
                using (var client = new TcpClient())
                using (cancel.Token.Register(() => { upstream.Close(); client.Close(); }))
                using (var ssl = await Connect(upstream, kind).ConfigureAwait(false)) {
                    byte[] ack = new byte[1];
                    await Exact(ssl, ack, 1).ConfigureAwait(false);
                    if (ack[0] != 1) throw new InvalidDataException();
                    await client.ConnectAsync(IPAddress.Loopback, targetPort).ConfigureAwait(false);
                    client.NoDelay = true;
                    await Drain(Encode(client.GetStream(), ssl), Decode(ssl, client), client, upstream, 300000).ConfigureAwait(false);
                }
            } catch {}
            if (!cancel.IsCancellationRequested) await Task.Delay(100, cancel.Token).ConfigureAwait(false);
        }
    }

    async Task Probe() {
        // 실제 게스트 loopback → TLS → 호스트 MySQL 경로를 확인한다.
        // 초기 greeting의 프로토콜 번호만 검사하며 DB 인증은 시도하지 않는다.
        using (var probe = new TcpClient())
        using (cancel.Token.Register(() => probe.Close())) {
            await probe.ConnectAsync(IPAddress.Loopback, GuestPort).ConfigureAwait(false);
            byte[] greeting = new byte[5];
            await Exact(probe.GetStream(), greeting, greeting.Length).ConfigureAwait(false);
            if (greeting[3] != 0 || greeting[4] != 10) throw new InvalidDataException();
        }
    }

    public async Task Run() {
        using (var tcp = new TcpClient())
        using (cancel.Token.Register(() => tcp.Close())) {
            try {
                var opening = Connect(tcp, 0);
                if (await Task.WhenAny(opening, Task.Delay(10000)).ConfigureAwait(false) != opening) {
                    tcp.Close();
                    try { using (await opening.ConfigureAwait(false)) {} } catch {}
                    throw new TimeoutException();
                }
                using (var control = await opening.ConfigureAwait(false)) {
                    Task accepting = Accept();
                    // HTTP keep-alive를 포함해 동시에 웹 16개, 관리 4개로 상한을 둔다.
                    // 실행 목적지는 두 상수뿐이며 임의 주소/포트를 받지 않는다.
                    for (int i = 0; i < 16; i++) { Task web = Reverse(2, 8080); }
                    for (int i = 0; i < 4; i++) { Task admin = Reverse(3, 8090); }
                    Task probing = Probe();
                    if (await Task.WhenAny(probing, accepting, Task.Delay(10000)).ConfigureAwait(false) != probing) throw new TimeoutException();
                    await probing.ConfigureAwait(false);
                    byte[] selected = { (byte)(GuestPort >> 8), (byte)GuestPort };
                    await control.WriteAsync(selected, 0, 2, cancel.Token).ConfigureAwait(false);
                    byte[] heartbeat = new byte[1];
                    for (;;) {
                        Task reading = Exact(control, heartbeat, 1);
                        Task first = await Task.WhenAny(reading, accepting, Task.Delay(10000)).ConfigureAwait(false);
                        if (first != reading) throw new IOException();
                        await reading.ConfigureAwait(false);
                        if (heartbeat[0] != 0) throw new InvalidDataException();
                        await control.WriteAsync(heartbeat, 0, 1, cancel.Token).ConfigureAwait(false);
                    }
                }
            } finally { Dispose(); }
        }
    }

    public void Dispose() {
        if (!cancel.IsCancellationRequested) { cancel.Cancel(); listener.Stop(); Array.Clear(key, 0, key.Length); }
    }
}
