// Isolated Windows process tree and loopback HTTP fixture. No Studio or user data.
using System;
using System.Diagnostics;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
[assembly: System.Reflection.AssemblyFileVersion("11.12.6.0")]
class Fixture {
    static void Main(string[] args) {
        if (args.Length == 1 && args[0] == "java") { Runtime(); return; }
        Process child = null;
        try {
            Console.WriteLine("ready");
            using (var startSignal = EventWaitHandle.OpenExisting(args[0] + "-start"))
            using (var stopSignal = EventWaitHandle.OpenExisting(args[0] + "-stop"))
            using (var exitSignal = EventWaitHandle.OpenExisting(args[0] + "-exit")) {
            for (;;) {
                int command = WaitHandle.WaitAny(new WaitHandle[] { startSignal, stopSignal, exitSignal });
                if (command == 2) break;
                if (command == 1) {
                    if (child != null) { child.Kill(); child.WaitForExit(); child.Dispose(); child = null; }
                    Console.WriteLine("stopped");
                } else if (command == 0) {
                    if (child != null) throw new Exception();
                    var start = new ProcessStartInfo(Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "java.exe"), "java");
                    start.UseShellExecute = false; start.CreateNoWindow = true; start.RedirectStandardOutput = true;
                    start.EnvironmentVariables["M2EE_ADMIN_PASS"] = Guid.NewGuid().ToString("N") + "é한글";
                    child = Process.Start(start);
                    if (child.StandardOutput.ReadLine() != "ready") throw new Exception();
                    Console.WriteLine("started");
                }
            }
            }
        } finally { if (child != null) { try { child.Kill(); child.WaitForExit(); } catch {} child.Dispose(); } }
    }
    static void Runtime() {
        var listener = new TcpListener(IPAddress.Loopback, 8090); listener.Server.ExclusiveAddressUse = true; listener.Start();
        Console.WriteLine("ready");
        for (;;) {
            using (var client = listener.AcceptTcpClient()) {
                client.ReceiveTimeout = 5000; client.SendTimeout = 5000;
                using (var reader = new StreamReader(client.GetStream(), Encoding.ASCII, false, 4096, true)) {
                    string first = reader.ReadLine(), line, authentication = null; int length = 0;
                    if (first == null) continue; // Ownership refusal closes before sending a header.
                    while (!String.IsNullOrEmpty(line = reader.ReadLine())) {
                        int colon = line.IndexOf(':'); string key = line.Substring(0, colon), value = line.Substring(colon+1).Trim();
                        if (key.Equals("Content-Length", StringComparison.OrdinalIgnoreCase)) length = Int32.Parse(value);
                        if (key.Equals("X-M2EE-Authentication", StringComparison.OrdinalIgnoreCase)) authentication = value;
                    }
                    char[] body = new char[length]; int offset = 0;
                    while (offset < length) { int got = reader.Read(body, offset, length-offset); if (got <= 0) throw new Exception(); offset += got; }
                    bool authorized = authentication == Convert.ToBase64String(Encoding.UTF8.GetBytes(Environment.GetEnvironmentVariable("M2EE_ADMIN_PASS")));
                    bool servlet = first.StartsWith("POST /dev/preview_execute_oql ", StringComparison.Ordinal);
                    string payload = servlet ? (authorized ? "{\"data\":[{\"Code\":\"en_US\"}]}" : "{}") : (authorized ? "{\"result\":0}" : "{\"result\":-4}");
                    byte[] response = Encoding.ASCII.GetBytes("HTTP/1.1 " + (servlet && !authorized ? "403 Forbidden" : "200 OK") + "\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: " + payload.Length + "\r\n\r\n" + payload);
                    client.GetStream().Write(response, 0, response.Length);
                }
            }
        }
    }
}
