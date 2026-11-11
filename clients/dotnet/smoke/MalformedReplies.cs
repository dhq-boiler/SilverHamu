using System.Buffers.Binary;
using System.Net;
using System.Net.Sockets;
using System.Text;
using SilverHamu;

internal static class MalformedReplies
{
    public static void Check()
    {
        foreach (var (write, payload) in new[]
        {
            (false, """{"ok":true}"""),
            (false, """{"ok":true,"value":42}"""),
            (false, """{"ok":true,"value":"zz"}"""),
            (true, """{"ok":true}"""),
            (true, """{"ok":true,"version":3}"""),
            (true, """{"ok":true,"version":"18446744073709551616"}"""),
        })
        {
            using var server = new TcpListener(IPAddress.Loopback, 0);
            server.Start();
            int port = ((IPEndPoint)server.LocalEndpoint).Port;
            Task peer = Task.Run(() =>
            {
                using TcpClient socket = server.AcceptTcpClient();
                using var stream = socket.GetStream();
                stream.ReadTimeout = 3000;
                Respond(stream, """{"ok":true,"protocol":1}""");
                Respond(stream, payload);
                if (stream.ReadByte() != -1) throw new Exception("Client kept a malformed session open");
            });
            using var client = SilverHamuClient.ConnectToken("127.0.0.1", port, "fixture");
            try
            {
                if (write) client.PutCommit([1], [2]); else client.GetLatest([1]);
                throw new Exception("Client accepted a malformed typed response");
            }
            catch (IOException error) when (error.Message.Contains("may have committed")) { }
            try
            {
                client.Ping();
                throw new Exception("Client reused an invalid session");
            }
            catch (ObjectDisposedException) { }
            peer.GetAwaiter().GetResult();
        }
        Console.WriteLine("C# rejected all six malformed typed replies and closed each connection");
    }

    private static void Respond(NetworkStream stream, string reply)
    {
        Span<byte> header = stackalloc byte[4];
        stream.ReadExactly(header);
        stream.ReadExactly(new byte[BinaryPrimitives.ReadInt32BigEndian(header)]);
        byte[] body = Encoding.UTF8.GetBytes(reply);
        BinaryPrimitives.WriteInt32BigEndian(header, body.Length);
        stream.Write(header);
        stream.Write(body);
    }
}
