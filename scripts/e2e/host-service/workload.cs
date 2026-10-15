using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Net.Sockets;
using System.IO;
using System.Text;
using System.Threading.Tasks;
using System.Threading;

// 실제 published REST microflow만 호출한다. DB 직접 쓰기나 readiness 측정은 없다.
public static class RelayWorkload {
    static readonly HttpClient client = Create();
    public static string Stage = "initializing";
    static HttpClient Create() {
        ServicePointManager.DefaultConnectionLimit = 16;
        var handler = new HttpClientHandler { UseProxy = false };
        return new HttpClient(handler) { BaseAddress = new Uri("http://127.0.0.1:8080/rest/relay/"), Timeout = TimeSpan.FromSeconds(120) };
    }
    static async Task<string> Call(string path, bool post = false, bool failure = false) {
        using (var request = new HttpRequestMessage(post ? HttpMethod.Post : HttpMethod.Get, path))
        using (var response = await client.SendAsync(request).ConfigureAwait(false)) {
            var text = (await response.Content.ReadAsStringAsync().ConfigureAwait(false)).Trim().Trim('"');
            if (failure ? response.IsSuccessStatusCode : !response.IsSuccessStatusCode) throw new Exception("unexpected HTTP result");
            return text;
        }
    }
    static void Equal(string got, string expected) { if (got != expected) throw new Exception("app result mismatch"); }
    static byte[] Packet(Stream stream) {
        var header = new byte[4]; Exact(stream, header);
        int size = header[0] | header[1] << 8 | header[2] << 16;
        if (size < 1 || size > 65536) throw new Exception("unexpected MySQL packet");
        var body = new byte[size]; Exact(stream, body); return body;
    }
    static void Exact(Stream stream, byte[] data) {
        int offset = 0;
        while (offset < data.Length) { int n = stream.Read(data, offset, data.Length - offset); if (n == 0) throw new EndOfStreamException(); offset += n; }
    }
    static void RejectInvalidDatabaseIdentity(int guestPort) {
        using (var tcp = new TcpClient()) {
            tcp.ReceiveTimeout = tcp.SendTimeout = 10000;
            tcp.Connect(IPAddress.Loopback, guestPort);
            var stream = tcp.GetStream();
            if (Packet(stream)[0] != 10) throw new Exception("missing greeting");
            using (var buffer = new MemoryStream())
            using (var writer = new BinaryWriter(buffer)) {
                writer.Write((uint)(1 | 4 | 512 | 32768 | 524288)); // protocol41, secure connection, plugin auth
                writer.Write((uint)16777216); writer.Write((byte)45); writer.Write(new byte[23]);
                writer.Write(Encoding.ASCII.GetBytes("mendimaru_invalid_acceptance\0"));
                writer.Write((byte)0); writer.Write(Encoding.ASCII.GetBytes("caching_sha2_password\0"));
                var data = buffer.ToArray();
                stream.Write(new byte[] { (byte)data.Length, (byte)(data.Length >> 8), 0, 1 }, 0, 4);
                stream.Write(data, 0, data.Length);
                var reply = Packet(stream);
                // 없는 계정에도 plugin 협상이 일어난다. 비밀값 없이 빈 응답을 보낸다.
                if (reply[0] == 254) {
                    stream.Write(new byte[] { 0, 0, 0, 3 }, 0, 4);
                    reply = Packet(stream);
                } else if (reply[0] == 1) {
                    stream.Write(new byte[] { 1, 0, 0, 3, 0 }, 0, 5);
                    reply = Packet(stream);
                }
                Stage = "invalid DB authentication reply=" + reply[0] + " code=" + (reply.Length > 2 ? (reply[1] | reply[2] << 8).ToString() : "short");
                if (reply[0] != 255 || reply.Length < 3 || (reply[1] | reply[2] << 8) != 1045) throw new Exception("invalid DB identity was not rejected");
            }
        }
    }
    public static async Task<Dictionary<string, object>> Acceptance(int guestPort) {
        Stage = "connection evidence";
        string evidence = await Call("evidence").ConfigureAwait(false);
        if (!evidence.StartsWith("8.4.") || !evidence.Contains("|READ-COMMITTED|ROW|") || !evidence.EndsWith("|InnoDB")) throw new Exception("connection preconditions");
        // 서로 다른 HTTP transaction으로 commit과 예외에 의한 rollback을 확인한다.
        Stage = "reset acceptance"; Equal(await Call("reset?Count=0", true), "0");
        Stage = "initial read"; Equal(await Call("rows?Sequence=20001"), "0");
        Stage = "write and commit"; Equal(await Call("rows?Sequence=20001", true), "20001");
        Equal(await Call("rows?Sequence=20001"), "1");
        Stage = "committed balance"; Equal(await Call("balance?Sequence=20001"), "101.25");
        Stage = "rollback"; await Call("rollback?Sequence=20002", true, true);
        Equal(await Call("rows?Sequence=20002"), "0");
        Stage = "invalid DB authentication"; RejectInvalidDatabaseIdentity(guestPort);
        Stage = "acceptance complete";
        return new Dictionary<string, object> { {"connection", evidence}, {"writeReadCommit", true}, {"errorRollback", true}, {"invalidDbAuthenticationRejected", true} };
    }
    public static async Task<int> Reset() {
        Stage = "seed 10000 rows";
        Equal(await Call("reset?Count=10000", true), "10000");
        Equal(await Call("rows?Sequence=9999"), "1");
        Equal(await Call("balance?Sequence=9999"), "101.25");
        return 10000;
    }
    static async Task SampleMemory(int runtimePid, int relayPid, CancellationToken cancel, List<long> runtimeSamples, List<long> relaySamples) {
        using (var runtime = Process.GetProcessById(runtimePid))
        using (var relay = Process.GetProcessById(relayPid)) {
            try {
                while (!cancel.IsCancellationRequested) {
                    runtime.Refresh(); relay.Refresh();
                    runtimeSamples.Add(runtime.WorkingSet64); relaySamples.Add(relay.WorkingSet64);
                    await Task.Delay(100, cancel).ConfigureAwait(false);
                }
            } catch (OperationCanceledException) {}
        }
    }
    static Dictionary<string, object> MemoryStats(List<long> samples) {
        return new Dictionary<string, object> {
            {"meanBytes", samples.Average()}, {"peakBytes", samples.Max()},
            {"samples", samples.Count}, {"seriesBytes", samples.ToArray()}
        };
    }
    public static async Task<Dictionary<string, object>> Run(int iterations, int runtimePid, int relayPid) {
        Stage = "paired workload";
        var runtimeSamples = new List<long>(); var relaySamples = new List<long>();
        var sampling = new CancellationTokenSource();
        Task sampler = runtimePid > 0 && relayPid > 0
            ? SampleMemory(runtimePid, relayPid, sampling.Token, runtimeSamples, relaySamples)
            : Task.FromResult(0);
        var elapsed = Stopwatch.StartNew();
        var lanes = Enumerable.Range(0, 4).Select(async lane => {
            var times = new List<double>();
            for (int i = 0; i < iterations; ++i) {
                int sequence = lane * 2500 + (i * 37 % 2500);
                foreach (string resource in new[] { "rows", "update", "balance" }) {
                    var watch = Stopwatch.StartNew();
                    string result = await Call(resource + "?Sequence=" + sequence, resource == "update").ConfigureAwait(false);
                    if (resource == "rows") Equal(result, "1");
                    else if (Decimal.Parse(result, System.Globalization.CultureInfo.InvariantCulture) < 102.25m) throw new Exception("update was not committed");
                    times.Add(watch.Elapsed.TotalMilliseconds);
                }
            }
            return times;
        }).ToArray();
        var completion = Task.WhenAll(lanes);
        await Task.WhenAny(completion).ConfigureAwait(false);
        elapsed.Stop();
        sampling.Cancel();
        await sampler.ConfigureAwait(false);
        sampling.Dispose();
        var results = await completion.ConfigureAwait(false);
        var sorted = results.SelectMany(x => x).OrderBy(x => x).ToArray();
        var report = new Dictionary<string, object> {
            {"lanes", 4}, {"iterationsPerLane", iterations}, {"requests", sorted.Length}, {"seconds", elapsed.Elapsed.TotalSeconds},
            {"requestsPerSecond", sorted.Length / elapsed.Elapsed.TotalSeconds}, {"meanMs", sorted.Average()},
            {"p50Ms", sorted[(int)Math.Ceiling(sorted.Length * .50) - 1]},
            {"p95Ms", sorted[(int)Math.Ceiling(sorted.Length * .95) - 1]},
            {"p99Ms", sorted[(int)Math.Ceiling(sorted.Length * .99) - 1]}
        };
        if (runtimeSamples.Count > 0) {
            report["memory"] = new Dictionary<string, object> {
                {"sampleIntervalMs", 100}, {"runtime", MemoryStats(runtimeSamples)}, {"guestRelay", MemoryStats(relaySamples)}
            };
        }
        return report;
    }
}
