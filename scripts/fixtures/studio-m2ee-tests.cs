namespace MendimaruRuntime {
    using System;
    using System.IO;
    using System.Text;
    using System.Threading;
    using System.Threading.Tasks;
    public static class BridgeTests {
        sealed class Duplex : Stream {
            readonly MemoryStream input;
            public readonly MemoryStream Output = new MemoryStream();
            public Duplex(byte[] bytes) { input = new MemoryStream(bytes); }
            public override bool CanRead { get { return true; } } public override bool CanWrite { get { return true; } } public override bool CanSeek { get { return false; } }
            public override long Length { get { throw new NotSupportedException(); } } public override long Position { get { throw new NotSupportedException(); } set { throw new NotSupportedException(); } }
            public override void Flush() {} public override long Seek(long n, SeekOrigin o) { throw new NotSupportedException(); } public override void SetLength(long n) { throw new NotSupportedException(); }
            public override int Read(byte[] b, int o, int n) { return input.Read(b,o,n); }
            public override void Write(byte[] b, int o, int n) { Output.Write(b,o,n); }
        }
        static byte[] Framed(string text, int fragment) {
            byte[] bytes = Encoding.ASCII.GetBytes(text);
            using (var stream = new MemoryStream()) {
                for (int offset=0; offset<bytes.Length;) {
                    int n=Math.Min(fragment,bytes.Length-offset);
                    byte[] h={(byte)(n>>24),(byte)(n>>16),(byte)(n>>8),(byte)n};stream.Write(h,0,4);stream.Write(bytes,offset,n);offset+=n;
                }
                return stream.ToArray();
            }
        }
        static string Unframed(byte[] bytes) {
            using (var result=new MemoryStream()) {
                int offset=0;
                while(offset<bytes.Length) {
                    if(offset+4>bytes.Length)throw new Exception("incomplete frame");
                    int n=(bytes[offset]<<24)|(bytes[offset+1]<<16)|(bytes[offset+2]<<8)|bytes[offset+3];offset+=4;
                    if(n==0) { if(offset!=bytes.Length)throw new Exception("trailing frame"); return Encoding.ASCII.GetString(result.ToArray()); }
                    if(n<0 || n>16384 || offset+n>bytes.Length)throw new Exception("invalid frame");
                    result.Write(bytes,offset,n);offset+=n;
                }
                throw new Exception("missing EOF");
            }
        }
        static string Request(string path,string auth,string body) { return "POST "+path+" HTTP/1.1\r\nHost: localhost\r\nContent-Length: "+body.Length+"\r\n"+(auth==null?"":"X-M2EE-Authentication: "+auth+"\r\n")+"\r\n"+body; }
        static string Send(AdminBridge bridge,string request,int fragment) {
            using(var wire=new Duplex(Framed(request,fragment))) { bridge.Serve(wire,CancellationToken.None).GetAwaiter().GetResult();return Unframed(wire.Output.ToArray()); }
        }
        static void Reject(AdminBridge bridge,string request) {
            bool rejected=false; try { Send(bridge,request,5); } catch { rejected=true; }
            if(!rejected)throw new Exception("unsafe input accepted");
        }
        public static void Verify(int pid,long ticks,string path,string password) {
            using(var bridge=new AdminBridge(pid,ticks,path,password)) {
                string alias=Convert.ToBase64String(Encoding.UTF8.GetBytes(password));
                foreach(string auth in new string[]{null,"incorrect",alias}) {
                    bool valid=auth==alias;
                    string servlet=Send(bridge,Request("/dev/preview_execute_oql",auth,"{}"),1);
                    if(!servlet.StartsWith(valid?"HTTP/1.1 200 ":"HTTP/1.1 403 ") || (valid&&!servlet.Contains("\"data\"")))throw new Exception("servlet authentication failed");
                    string status=Send(bridge,Request("/",auth,"{\"action\":\"runtime_status\",\"params\":{}}"),7);
                    if(!status.Contains(valid?"\"result\":0":"\"result\":-4"))throw new Exception("M2EE result lost");
                }
                string shutdown=Send(bridge,Request("/",alias,"{\"action\":\"shutdown\",\"params\":{}}"),1024);
                if(!shutdown.Contains("\"result\":0"))throw new Exception("shutdown failed");
                Reject(bridge,Request("http://example.com/",alias,"{}"));
                Reject(bridge,Request("/other",alias,"{}"));
                Reject(bridge,"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
                Reject(bridge,"POST / HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n");
                Reject(bridge,"POST / HTTP/1.1\r\nContent-Length: 1048577\r\n\r\n");
                Reject(bridge,"POST / HTTP/1.1\r\nContent-Length: 0\r\n X-M2EE-Authentication: x\r\n\r\n");
                Reject(bridge,Request("/",alias,"{}")+Request("/",alias,"{}"));
            }
            bool refused=false;try{using(var bridge=new AdminBridge(pid,ticks+1,path,password)) {}}catch{refused=true;}
            if(!refused)throw new Exception("stale Studio identity accepted");
        }
        public static void RefuseForeignListener(int pid,long ticks,string path,string password) {
            using(var bridge=new AdminBridge(pid,ticks,path,password)) { Reject(bridge,Request("/",Convert.ToBase64String(Encoding.UTF8.GetBytes(password)),"{}")); }
        }
    }
}
