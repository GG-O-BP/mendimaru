// Fixed-purpose Studio 11.12.6 admin authentication bridge. No credential API,
// arbitrary PID, address, file, environment name or script is exposed to callers.
namespace MendimaruRuntime {
    using System;
    using System.Collections.Generic;
    using System.Diagnostics;
    using System.IO;
    using System.Net;
    using System.Net.Sockets;
    using System.Runtime.InteropServices;
    using System.Text;
    using System.Threading;
    using System.Threading.Tasks;

    internal sealed class AdminBridge : IDisposable {
        readonly int studioPid;
        readonly long studioTicks;
        readonly string studioPath;
        readonly byte[] alias;
        const int MaxHeader = 16384, MaxBody = 1048576;
        public AdminBridge(int pid, long ticks, string path, string password) {
            studioPid = pid; studioTicks = ticks; studioPath = path;
            if (String.IsNullOrEmpty(password) || Encoding.UTF8.GetByteCount(password) > 256) throw new InvalidDataException();
            foreach (char c in password) if (Char.IsControl(c)) throw new InvalidDataException();
            alias = Encoding.ASCII.GetBytes(Convert.ToBase64String(Encoding.UTF8.GetBytes(password)));
            using (var studio = Studio()) {}
        }
        Process Studio() {
            var p = Process.GetProcessById(studioPid);
            try {
                if (p.ProcessName.ToLowerInvariant() != "studiopro" || p.StartTime.ToUniversalTime().Ticks != studioTicks ||
                    !String.Equals(p.MainModule.FileName, studioPath, StringComparison.OrdinalIgnoreCase) ||
                    p.MainModule.FileVersionInfo.FileVersion != "11.12.6.0" || !Native.SameOwner(p.Id)) throw new InvalidDataException();
                return p;
            } catch { p.Dispose(); throw; }
        }
        bool Matches(string header) {
            byte[] actual = Encoding.ASCII.GetBytes(header);
            int difference = actual.Length ^ alias.Length;
            for (int i = 0; i < alias.Length; i++) difference |= alias[i] ^ (i < actual.Length ? actual[i] : 0);
            Array.Clear(actual, 0, actual.Length);
            return difference == 0;
        }
        string Password(int pid, int peerPort) {
            using (var studio = Studio()) {
                if (Native.AdminListener() != pid) throw new InvalidDataException();
                using (var child = Process.GetProcessById(pid)) {
                    if ((child.ProcessName != "java" && child.ProcessName != "javaw") || child.SessionId != studio.SessionId ||
                        child.StartTime.ToUniversalTime().Ticks < studioTicks || !Native.SameOwner(pid)) throw new InvalidDataException();
                    // Require an unbroken, live, monotonically ordered ancestry.
                    int current = pid; long previous = child.StartTime.ToUniversalTime().Ticks;
                    bool owned = false;
                    for (int depth = 0; depth < 8; depth++) {
                        int parent = Native.Parent(current);
                        if (parent == studioPid) { owned = true; break; }
                        if (parent <= 0 || parent == current) break;
                        using (var ancestor = Process.GetProcessById(parent)) {
                            long started = ancestor.StartTime.ToUniversalTime().Ticks;
                            if (started < studioTicks || started > previous || ancestor.SessionId != studio.SessionId || !Native.SameOwner(parent)) break;
                            previous = started; current = parent;
                        }
                    }
                    if (!owned) throw new InvalidDataException();
                    string value = Native.M2eePassword(child.Id);
                    // Recheck the listener and identities after reading; never cache a Runtime secret across F5.
                    if (Native.AdminListener() != child.Id || Native.AdminConnectionOwner(peerPort) != child.Id || child.HasExited) throw new InvalidDataException();
                    using (var checkedStudio = Studio()) {}
                    return value;
                }
            }
        }
        static async Task<byte[]> Frame(Stream wire, CancellationToken token) {
            byte[] h = new byte[4]; await Exact(wire, h, token).ConfigureAwait(false);
            uint n = ((uint)h[0] << 24) | ((uint)h[1] << 16) | ((uint)h[2] << 8) | h[3];
            if (n > 16384) throw new InvalidDataException();
            byte[] data = new byte[n]; await Exact(wire, data, token).ConfigureAwait(false); return data;
        }
        static async Task Exact(Stream wire, byte[] bytes, CancellationToken token) {
            int n = 0;
            while (n < bytes.Length) { int read = await wire.ReadAsync(bytes, n, bytes.Length - n, token).ConfigureAwait(false); if (read == 0) throw new EndOfStreamException(); n += read; }
        }
        static async Task Send(Stream wire, byte[] bytes, CancellationToken token) {
            for (int offset = 0; offset < bytes.Length;) {
                int n = Math.Min(16384, bytes.Length - offset);
                byte[] h = { (byte)(n >> 24), (byte)(n >> 16), (byte)(n >> 8), (byte)n };
                await wire.WriteAsync(h, 0, 4, token).ConfigureAwait(false);
                await wire.WriteAsync(bytes, offset, n, token).ConfigureAwait(false); offset += n;
            }
        }
        // One bounded POST per connection. Do not tunnel keep-alive/pipelining,
        // chunked input, CONNECT, redirects, proxy settings or arbitrary headers.
        public async Task Serve(Stream wire, CancellationToken token) {
            using (var deadline = CancellationTokenSource.CreateLinkedTokenSource(token)) {
                deadline.CancelAfter(15000); token = deadline.Token;
                using (var input = new MemoryStream()) {
                    int headerEnd = -1, length = -1; string path = null, auth = null;
                    while (headerEnd < 0 || input.Length < headerEnd + length) {
                        byte[] frame = await Frame(wire, token).ConfigureAwait(false);
                        if (frame.Length == 0 || input.Length + frame.Length > MaxHeader + MaxBody) throw new InvalidDataException();
                        input.Write(frame, 0, frame.Length); Array.Clear(frame, 0, frame.Length);
                        byte[] bytes = input.GetBuffer();
                        if (headerEnd < 0) {
                            for (int i = 0; i + 3 < input.Length && i < MaxHeader; i++) {
                                if (bytes[i] == 13 && bytes[i+1] == 10 && bytes[i+2] == 13 && bytes[i+3] == 10) { headerEnd = i + 4; break; }
                            }
                            if (headerEnd < 0) { if (input.Length >= MaxHeader) throw new InvalidDataException(); continue; }
                            if (headerEnd > MaxHeader) throw new InvalidDataException();
                            string[] lines = Encoding.ASCII.GetString(bytes, 0, headerEnd - 4).Split(new string[] { "\r\n" }, StringSplitOptions.None);
                            string[] first = lines[0].Split(' ');
                            if (first.Length != 3 || first[0] != "POST" || first[2] != "HTTP/1.1" ||
                                (first[1] != "/" && first[1] != "/dev/preview_execute_oql")) throw new InvalidDataException();
                            path = first[1]; var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                            for (int i = 1; i < lines.Length; i++) {
                                int colon = lines[i].IndexOf(':');
                                if (colon <= 0) throw new InvalidDataException();
                                string name = lines[i].Substring(0, colon), value = lines[i].Substring(colon + 1).Trim();
                                foreach (char c in name) if (!(Char.IsLetterOrDigit(c) && c < 128) && c != '-') throw new InvalidDataException();
                                foreach (char c in value) if (Char.IsControl(c)) throw new InvalidDataException();
                                if (!seen.Add(name) || name.Equals("Transfer-Encoding", StringComparison.OrdinalIgnoreCase) || name.Equals("Expect", StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException();
                                if (name.Equals("Content-Length", StringComparison.OrdinalIgnoreCase) && (!Int32.TryParse(value, out length) || length < 0 || length > MaxBody)) throw new InvalidDataException();
                                if (name.Equals("X-M2EE-Authentication", StringComparison.OrdinalIgnoreCase)) auth = value;
                            }
                            if (length < 0) throw new InvalidDataException();
                        }
                        if (input.Length > headerEnd + length) throw new InvalidDataException();
                    }
                    // Connect first and authenticate the owner of this exact
                    // accepted socket. A listener replacement cannot receive a
                    // credential through reconnect, connection pooling or a race.
                    using (var tcp = new TcpClient())
                    using (token.Register(() => tcp.Close())) {
                        await tcp.ConnectAsync(IPAddress.Loopback, 8090).ConfigureAwait(false);
                        int peerPort = ((IPEndPoint)tcp.Client.LocalEndPoint).Port;
                        string secret = Password(Native.AdminConnectionOwner(peerPort), peerPort);
                        string translated = auth == null ? null : (Matches(auth) ? Convert.ToBase64String(Encoding.UTF8.GetBytes(secret)) : Convert.ToBase64String(Guid.NewGuid().ToByteArray()));
                        secret = null; auth = null;
                        byte[] header = Encoding.ASCII.GetBytes("POST " + path + " HTTP/1.1\r\nHost: 127.0.0.1:8090\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: " + length.ToString(System.Globalization.CultureInfo.InvariantCulture) + "\r\n" +
                            (translated == null ? "" : "X-M2EE-Authentication: " + translated + "\r\n") + "\r\n");
                        translated = null;
                        try {
                            Stream upstream = tcp.GetStream();
                            await upstream.WriteAsync(header, 0, header.Length, token).ConfigureAwait(false);
                            await upstream.WriteAsync(input.GetBuffer(), headerEnd, length, token).ConfigureAwait(false);
                            using (var output = new MemoryStream()) {
                                byte[] buffer = new byte[16384]; int n, responseHeaderEnd = -1;
                                while ((n = await upstream.ReadAsync(buffer, 0, buffer.Length, token).ConfigureAwait(false)) != 0) {
                                    if (output.Length + n > MaxHeader + MaxBody) throw new InvalidDataException();
                                    output.Write(buffer, 0, n);
                                    if (responseHeaderEnd < 0) {
                                        byte[] bytes = output.GetBuffer();
                                        for (int i = 0; i + 3 < output.Length && i + 4 <= MaxHeader; i++) {
                                            if (bytes[i] == 13 && bytes[i+1] == 10 && bytes[i+2] == 13 && bytes[i+3] == 10) { responseHeaderEnd = i+4; break; }
                                        }
                                        if (responseHeaderEnd < 0 && output.Length >= MaxHeader) throw new InvalidDataException();
                                    }
                                }
                                if (responseHeaderEnd < 0 || output.Length - responseHeaderEnd > MaxBody) throw new InvalidDataException();
                                string status = Encoding.ASCII.GetString(output.GetBuffer(), 0, Math.Min(13, (int)output.Length));
                                if ((!status.StartsWith("HTTP/1.1 ") && !status.StartsWith("HTTP/1.0 ")) || status.Length < 13 ||
                                    status[9] < '2' || status[9] > '5' || status[10] < '0' || status[10] > '9' || status[11] < '0' || status[11] > '9' || status[12] != ' ') throw new InvalidDataException();
                                await Send(wire, output.ToArray(), token).ConfigureAwait(false);
                                await wire.WriteAsync(new byte[4], 0, 4, token).ConfigureAwait(false);
                            }
                        } finally { Array.Clear(header, 0, header.Length); Array.Clear(input.GetBuffer(), 0, (int)input.Length); }
                    }
                }
            }
        }
        public void Dispose() { Array.Clear(alias, 0, alias.Length); }
    }

    internal static class Native {
        [StructLayout(LayoutKind.Sequential)] struct Basic { public IntPtr Exit, Peb, Affinity, Priority, Pid, Parent; }
        [DllImport("ntdll.dll")] static extern int NtQueryInformationProcess(IntPtr process, int kind, ref Basic basic, int size, out int returned);
        [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
        [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
        [DllImport("kernel32.dll")] static extern bool ReadProcessMemory(IntPtr process, IntPtr address, byte[] data, int size, out IntPtr read);
        [DllImport("kernel32.dll")] static extern bool IsWow64Process(IntPtr process, out bool wow64);
        [DllImport("advapi32.dll")] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
        [DllImport("iphlpapi.dll")] static extern uint GetExtendedTcpTable(IntPtr data, ref int size, bool order, int family, int tableClass, int reserved);
        static Basic Info(IntPtr process) { Basic b = new Basic(); int n; if (NtQueryInformationProcess(process, 0, ref b, Marshal.SizeOf(b), out n) != 0) throw new InvalidDataException(); return b; }
        public static int Parent(int pid) {
            IntPtr p = OpenProcess(0x1000, false, pid); if (p == IntPtr.Zero) throw new InvalidDataException();
            try { return checked((int)Info(p).Parent.ToInt64()); } finally { CloseHandle(p); }
        }
        public static bool SameOwner(int pid) {
            IntPtr p = OpenProcess(0x1000, false, pid), token = IntPtr.Zero;
            if (p == IntPtr.Zero) return false;
            try {
                if (!OpenProcessToken(p, 8, out token)) return false;
                using (var owner = new System.Security.Principal.WindowsIdentity(token)) {
                    return System.Security.Principal.WindowsIdentity.GetCurrent().User.Equals(owner.User);
                }
            } finally { if (token != IntPtr.Zero) CloseHandle(token); CloseHandle(p); }
        }
        public static int AdminListener() { return AdminOwner(0); }
        public static int AdminConnectionOwner(int peerPort) { if (peerPort <= 0) throw new InvalidDataException(); return AdminOwner(peerPort); }
        static int AdminOwner(int peerPort) {
            int table = peerPort == 0 ? 3 : 4;
            int size = 0; GetExtendedTcpTable(IntPtr.Zero, ref size, false, 2, table, 0);
            if (size < 4 || size > 1048576) throw new InvalidDataException();
            IntPtr data = Marshal.AllocHGlobal(size);
            try {
                if (GetExtendedTcpTable(data, ref size, false, 2, table, 0) != 0) throw new InvalidDataException();
                int count = Marshal.ReadInt32(data); if (count < 0 || count > (size - 4) / 24) throw new InvalidDataException();
                int found = 0;
                for (int i = 0; i < count; i++) {
                    IntPtr row = IntPtr.Add(data, 4 + i * 24);
                    int port = (Marshal.ReadByte(row, 8) << 8) | Marshal.ReadByte(row, 9);
                    if (Marshal.ReadInt32(row) != (peerPort == 0 ? 2 : 5) || port != 8090) continue;
                    if (peerPort != 0 && (((Marshal.ReadByte(row, 16) << 8) | Marshal.ReadByte(row, 17)) != peerPort || Marshal.ReadInt32(row, 12) != 0x0100007f)) continue;
                    // M2EE must retain guest IPv4 loopback isolation.
                    if (Marshal.ReadInt32(row, 4) != 0x0100007f || found != 0) throw new InvalidDataException();
                    found = Marshal.ReadInt32(row, 20);
                }
                if (found <= 0) throw new InvalidDataException(); return found;
            } finally { Marshal.FreeHGlobal(data); }
        }
        static byte[] Read(IntPtr process, long address, int length) {
            byte[] value = new byte[length]; IntPtr got;
            if (address <= 0 || !ReadProcessMemory(process, new IntPtr(address), value, length, out got) || got.ToInt64() != length) { Array.Clear(value, 0, value.Length); throw new InvalidDataException(); }
            return value;
        }
        public static string M2eePassword(int pid) {
            if (IntPtr.Size != 8) throw new InvalidDataException();
            IntPtr p = OpenProcess(0x410, false, pid); if (p == IntPtr.Zero) throw new InvalidDataException();
            byte[] environment = new byte[1048576];
            try {
                bool wow; if (!IsWow64Process(p, out wow) || wow) throw new InvalidDataException();
                byte[] pointer = Read(p, Info(p).Peb.ToInt64() + 0x20, 8);
                long parameters = BitConverter.ToInt64(pointer, 0); Array.Clear(pointer, 0, pointer.Length);
                pointer = Read(p, parameters + 0x80, 8);
                long address = BitConverter.ToInt64(pointer, 0); Array.Clear(pointer, 0, pointer.Length);
                int used = 0, start = 0; string result = null;
                while (used < environment.Length) {
                    // Stop at the terminator before crossing an unmapped region.
                    byte[] pair = Read(p, address + used, 2);
                    environment[used++] = pair[0]; environment[used++] = pair[1]; Array.Clear(pair, 0, pair.Length);
                    if (environment[used-2] != 0 || environment[used-1] != 0) continue;
                    if (used - start == 2) { if (String.IsNullOrEmpty(result)) throw new InvalidDataException(); return result; }
                    const string name = "M2EE_ADMIN_PASS=";
                    bool match = used - start - 2 >= name.Length * 2;
                    for (int i = 0; match && i < name.Length; i++) match = environment[start+i*2] == name[i] && environment[start+i*2+1] == 0;
                    if (match) {
                        if (result != null || used - start - 2 - name.Length*2 > 8192) throw new InvalidDataException();
                        result = new UnicodeEncoding(false, false, true).GetString(environment, start+name.Length*2, used-start-2-name.Length*2);
                    }
                    Array.Clear(environment, start, used-start); start = used;
                }
                throw new InvalidDataException();
            } finally { Array.Clear(environment, 0, environment.Length); CloseHandle(p); }
        }
    }
}
