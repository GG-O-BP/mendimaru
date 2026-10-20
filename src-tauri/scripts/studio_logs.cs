using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

// No shutdown/restart API is imported. Restart Manager is used only to prove
// the file is held by the exact Studio PID and creation time.
public static class MendimaruStudioLogFile {
    [StructLayout(LayoutKind.Sequential)] struct UniqueProcess { public int Id; public System.Runtime.InteropServices.ComTypes.FILETIME Started; }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct ProcessInfo {
        public UniqueProcess Process;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=256)] public string Name;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=64)] public string Service;
        public uint Type, Status, Session;
        [MarshalAs(UnmanagedType.Bool)] public bool Restartable;
    }
    [DllImport("rstrtmgr.dll", CharSet=CharSet.Unicode)] static extern int RmStartSession(out uint session, int flags, StringBuilder key);
    [DllImport("rstrtmgr.dll", CharSet=CharSet.Unicode)] static extern int RmRegisterResources(uint session, uint count, string[] files, uint applications, IntPtr apps, uint services, IntPtr serviceNames);
    [DllImport("rstrtmgr.dll")] static extern int RmGetList(uint session, out uint needed, ref uint count, [In, Out] ProcessInfo[] info, ref uint reasons);
    [DllImport("rstrtmgr.dll")] static extern int RmEndSession(uint session);
    [StructLayout(LayoutKind.Sequential)] struct FileInfo {
        public uint Attributes; public System.Runtime.InteropServices.ComTypes.FILETIME Created, Accessed, Written;
        public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
    }
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetFileInformationByHandle(SafeFileHandle handle, out FileInfo info);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern uint GetFinalPathNameByHandle(SafeFileHandle handle, StringBuilder path, uint length, uint flags);
    public static bool Owned(string path, int pid, long ticks) {
        uint session;
        if(RmStartSession(out session, 0, new StringBuilder(33)) != 0) throw new IOException();
        try {
            if(RmRegisterResources(session, 1, new string[]{path}, 0, IntPtr.Zero, 0, IntPtr.Zero) != 0) throw new IOException();
            uint needed, count=0, reasons=0;
            int result=RmGetList(session, out needed, ref count, null, ref reasons);
            if(result == 0 && needed == 0) return false;
            if(result != 234 || needed > 64) throw new IOException();
            var info=new ProcessInfo[needed]; count=needed;
            if(RmGetList(session, out needed, ref count, info, ref reasons) != 0) throw new IOException();
            bool found=false;
            for(int i=0;i<count;i++) {
                if(info[i].Process.Id == System.Diagnostics.Process.GetCurrentProcess().Id) continue;
                long filetime=((long)info[i].Process.Started.dwHighDateTime << 32) | (uint)info[i].Process.Started.dwLowDateTime;
                if(info[i].Process.Id != pid || DateTime.FromFileTimeUtc(filetime).Ticks != ticks) return false;
                found=true;
            }
            return found;
        } finally { RmEndSession(session); }
    }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern SafeFileHandle CreateFile(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
    public static FileStream Open(string path) {
        // OPEN_REPARSE_POINT + no delete sharing: never follow a substituted
        // link, and pin this file through ownership verification and reading.
        var handle=CreateFile(path, 0x80000000, 3, IntPtr.Zero, 3, 0x00200000, IntPtr.Zero);
        if(handle.IsInvalid) { handle.Dispose(); throw new IOException(); }
        var stream=new FileStream(handle, FileAccess.Read);
        try {
            FileInfo info; var finalPath=new StringBuilder(32768);
            if(!GetFileInformationByHandle(stream.SafeFileHandle, out info) || info.Links != 1 || (info.Attributes & (0x400u|0x10u)) != 0 ||
               GetFinalPathNameByHandle(stream.SafeFileHandle, finalPath, 32768, 0) == 0 ||
               !String.Equals(finalPath.ToString(), @"\\?\"+Path.GetFullPath(path), StringComparison.OrdinalIgnoreCase)) throw new IOException();
            return stream;
        } catch { stream.Dispose(); throw; }
    }
    public static string Identity(FileStream stream) {
        FileInfo info;
        if(!GetFileInformationByHandle(stream.SafeFileHandle, out info)) throw new IOException();
        return info.Volume.ToString("x8")+info.IndexHigh.ToString("x8")+info.IndexLow.ToString("x8");
    }
    public static string Fingerprint(FileStream stream, long end) {
        if(end < 0 || end > 16777216 || end > stream.Length) throw new IOException();
        long previous=stream.Position; stream.Position=0;
        try {
            using(var sha=System.Security.Cryptography.SHA256.Create()) {
                var buffer=new byte[65536]; long remaining=end;
                while(remaining > 0) {
                    int count=stream.Read(buffer, 0, (int)Math.Min(buffer.Length, remaining));
                    if(count == 0) throw new IOException();
                    sha.TransformBlock(buffer, 0, count, buffer, 0); remaining-=count;
                }
                sha.TransformFinalBlock(new byte[0], 0, 0);
                return BitConverter.ToString(sha.Hash).Replace("-", "").ToLowerInvariant();
            }
        } finally { stream.Position=previous; }
    }
    // Byte offsets remain exact across CRLF, UTF-8, and concurrent append.
    // Oversized/incomplete lines never expose a fragment of a secret.
    public static string Line(FileStream stream, long end, out bool oversized, out bool incomplete) {
        var bytes=new MemoryStream(); oversized=false; incomplete=false;
        while(stream.Position < end) {
            int value=stream.ReadByte(); if(value < 0) break;
            if(value == 10) {
                if(oversized) return "[REDACTED oversized log line]";
                return new UTF8Encoding(false,true).GetString(bytes.ToArray()).TrimEnd('\r').TrimStart('\uFEFF');
            }
            if(bytes.Length < 65536) bytes.WriteByte((byte)value); else oversized=true;
        }
        incomplete=true; return null;
    }
}
