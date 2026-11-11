using System.Buffers.Binary;
using System.Net.Sockets;
using SilverHamu.Sql;
using System.Text.Json.Nodes;

namespace SilverHamu.Owner.Services;

public sealed class SilverHamuClient : IDisposable
{
    private readonly TcpClient socket = new();
    public bool Connected => socket.Connected;
    public bool SupportsSql { get; private set; }
    public string? ReadRole { get; private set; }
    public bool SqlTransactionActive { get; private set; }

    public async Task ConnectAsync(string host, int port, string token)
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        await socket.ConnectAsync(host, port, deadline.Token);
        var response = await RequestAsync(new JsonObject { ["op"] = "auth", ["version"] = 1, ["token"] = token });
        SupportsSql = response["sql"]?.GetValue<bool>() == true;
        ReadRole = response["read_role"]?.GetValue<string>();
    }

    public async Task<JsonObject> RequestAsync(JsonObject request)
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        var body = System.Text.Encoding.UTF8.GetBytes(request.ToJsonString());
        if (body.Length > 1024 * 1024) throw new InvalidOperationException("要求は1 MiB以下にしてください。");
        var header = new byte[4];
        BinaryPrimitives.WriteInt32BigEndian(header, body.Length);
        try
        {
            var stream = socket.GetStream();
            await stream.WriteAsync(header, deadline.Token);
            await stream.WriteAsync(body, deadline.Token);
            await stream.ReadExactlyAsync(header, deadline.Token);
            var size = BinaryPrimitives.ReadInt32BigEndian(header);
            if (size is <= 0 or > 2105344) throw new IOException("応答サイズが不正です。");
            var bytes = new byte[size];
            await stream.ReadExactlyAsync(bytes, deadline.Token);
            var result = JsonNode.Parse(bytes) as JsonObject ?? throw new IOException("応答が不正です。");
            if (request["op"]?.ToString() == "sql")
                SqlTransactionActive = result["in_transaction"]?.GetValue<bool>() ?? throw new IOException("トランザクション状態が不明です。");
            if (result["ok"]?.GetValue<bool>() != true)
                throw new SilverHamuRequestException(result["error"]?.ToString() ?? "unknown_error");
            return result;
        }
        catch (SilverHamuRequestException) { throw; }
        catch
        {
            socket.Dispose();
            throw new IOException(request["op"]?.ToString() is "commit" or "sql"
                ? "確定結果が不明です。再送せず、再接続してデータを照合してください。"
                : "通信が中断しました。再接続してください。");
        }
    }

    public void Dispose() => socket.Dispose();
}


