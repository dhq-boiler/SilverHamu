using System.Buffers.Binary;
using System.Collections.Concurrent;
using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using System.Text.Json.Nodes;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ServiceConnectionTests
{
    [Fact]
    public async Task Kv_only_connections_keep_raw_requests_available_after_the_sql_capability_check()
    {
        await using var peer = new Peer(Auth(), Probe(Error("sql_unavailable")), new("ping", Json("""{"ok":true,"version":"19"}""")));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");

        Assert.True(connection.Connected);
        Assert.False(connection.SupportsSql);
        Assert.Null(connection.ReadRole);
        Assert.False(connection.SqlTransactionActive);
        Assert.Equal("19", (await connection.RequestAsync(Json("""{"op":"ping"}""")))["version"]!.GetValue<string>());
        await peer.Completion;
    }

    [Theory]
    [InlineData(null)]
    [InlineData("published")]
    public async Task Sql_connections_preserve_capability_and_the_authenticated_view_role(string? role)
    {
        await using var peer = new Peer(Auth(), Probe(Session(false, role)));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");

        Assert.True(connection.Connected);
        Assert.True(connection.SupportsSql);
        Assert.Equal(role, connection.ReadRole);
        Assert.False(connection.SqlTransactionActive);
        await peer.Completion;
    }

    [Fact]
    public async Task Rejected_authentication_preserves_the_server_error_and_does_not_open_a_session()
    {
        await using var peer = new Peer(new Exchange("auth", Error("unauthorized")));
        using var connection = new ServiceConnection();

        var error = await Assert.ThrowsAsync<SilverHamuRequestException>(() => connection.ConnectAsync("127.0.0.1", peer.Port, "fixture"));

        Assert.Equal("unauthorized", error.Message);
        Assert.False(connection.Connected);
        await peer.Completion;
        Assert.Single(peer.Requests);
    }

    [Theory]
    [InlineData(false, true)]
    [InlineData(true, false)]
    public async Task Failed_multi_statement_sql_refreshes_transaction_state_without_replaying_the_statement(bool before, bool after)
    {
        var initial = before ? "BEGIN" : "SELECT 1";
        var failing = before ? "COMMIT; SELECT missing" : "BEGIN; SELECT missing";
        var failure = Error("unknown_table");
        failure["in_transaction"] = after;
        await using var peer = new Peer(Auth(), Probe(Session(false)), new("sql", Session(before), initial),
            new("sql", failure, failing), Probe(Session(after)));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");
        await connection.RequestAsync(Sql(initial));
        Assert.Equal(before, connection.SqlTransactionActive);

        var error = await Assert.ThrowsAsync<SilverHamuRequestException>(() => connection.RequestAsync(Sql(failing)));

        Assert.Equal("unknown_table", error.Message);
        Assert.Equal(after, connection.SqlTransactionActive);
        Assert.True(connection.Connected);
        await peer.Completion;
        Assert.Single(peer.Requests, request => request["sql"]?.ToString() == failing);
    }

    [Theory]
    [InlineData("commit_uncertain")]
    [InlineData("checkpoint_uncertain")]
    [InlineData("reopen_required")]
    [InlineData("unauthorized")]
    [InlineData("sql_host_failed_outcome_unknown")]
    public async Task Fatal_server_errors_close_the_session_without_sending_a_state_probe(string code)
    {
        await using var peer = new Peer(Auth(), Probe(Session(false)), new("sql", Error(code), "INSERT INTO work VALUES (1)"));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");

        var error = await Assert.ThrowsAsync<SilverHamuRequestException>(() => connection.RequestAsync(Sql("INSERT INTO work VALUES (1)")));

        Assert.Equal(code, error.Message);
        Assert.False(connection.Connected);
        Assert.False(connection.SqlTransactionActive);
        await Assert.ThrowsAsync<InvalidOperationException>(() => connection.RequestAsync(Json("""{"op":"ping"}""")));
        await peer.Completion;
        Assert.Equal(3, peer.Requests.Count);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task Unavailable_or_malformed_transaction_state_closes_the_session(bool malformed)
    {
        var probe = malformed ? Json("""{"ok":true,"results":[]}""") : Error("sql_host_unavailable");
        await using var peer = new Peer(Auth(), Probe(Session(false)), new("sql", Error("unknown_table"), "SELECT missing"), Probe(probe));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");

        var error = await Assert.ThrowsAsync<IOException>(() => connection.RequestAsync(Sql("SELECT missing")));

        Assert.Contains("再接続", error.Message);
        Assert.False(connection.Connected);
        await peer.Completion;
    }

    [Fact]
    public async Task Malformed_initial_sql_metadata_is_not_accepted_as_a_usable_connection()
    {
        await using var peer = new Peer(Auth(), Probe(Json("""{"ok":true,"results":[]}""")));
        using var connection = new ServiceConnection();

        await Assert.ThrowsAsync<IOException>(() => connection.ConnectAsync("127.0.0.1", peer.Port, "fixture"));

        Assert.False(connection.Connected);
        await peer.Completion;
    }

    [Theory]
    [InlineData("ping", false)]
    [InlineData("commit", true)]
    [InlineData("sql", true)]
    public async Task Interrupted_exchanges_close_the_session_and_identify_uncertain_write_outcomes(string operation, bool uncertain)
    {
        await using var peer = new Peer(Auth(), Probe(operation == "sql" ? Session(false) : Error("sql_unavailable")), new(operation, null));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");
        var request = operation == "sql" ? Sql("INSERT INTO work VALUES (1)") : new JsonObject { ["op"] = operation };

        var error = await Assert.ThrowsAsync<IOException>(() => connection.RequestAsync(request));

        Assert.Equal(uncertain, error.Message.Contains("確定結果が不明"));
        Assert.False(connection.Connected);
        await peer.Completion;
        Assert.Equal(3, peer.Requests.Count);
    }

    [Theory]
    [InlineData(-1)]
    [InlineData(0)]
    [InlineData(1)]
    public async Task Request_size_limits_reject_only_oversized_messages_and_leave_the_session_usable(int extraBytes)
    {
        const int limit = 1024 * 1024;
        var request = new JsonObject { ["op"] = "ping", ["padding"] = "" };
        request["padding"] = new string('x', limit - JsonSerializer.SerializeToUtf8Bytes(request).Length + extraBytes);
        var ping = new Exchange("ping", Json("""{"ok":true,"version":"1"}"""));
        var exchanges = new List<Exchange> { Auth(), Probe(Error("sql_unavailable")) };
        if (extraBytes <= 0) exchanges.Add(ping);
        exchanges.Add(ping);
        await using var peer = new Peer(exchanges.ToArray());
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");

        if (extraBytes > 0)
            await Assert.ThrowsAsync<InvalidOperationException>(() => connection.RequestAsync(request));
        else
            await connection.RequestAsync(request);

        Assert.True(connection.Connected);
        await connection.RequestAsync(Json("""{"op":"ping"}"""));
        await peer.Completion;
    }

    [Fact]
    public async Task Concurrent_async_requests_complete_without_interleaving_wire_frames()
    {
        await using var peer = new Peer(Auth(), Probe(Error("sql_unavailable")),
            new("ping", Json("""{"ok":true,"version":"1"}""")), new("ping", Json("""{"ok":true,"version":"2"}""")));
        using var connection = new ServiceConnection();
        await connection.ConnectAsync("127.0.0.1", peer.Port, "fixture");

        var results = await Task.WhenAll(connection.RequestAsync(Json("""{"op":"ping"}""")), connection.RequestAsync(Json("""{"op":"ping"}""")));

        Assert.Equal(new[] { "1", "2" }, results.Select(result => result["version"]!.ToString()).Order());
        await peer.Completion;
    }

    [Fact]
    public async Task Disposed_connections_reject_connection_and_request_attempts()
    {
        var connection = new ServiceConnection();
        connection.Dispose();
        connection.Dispose();

        await Assert.ThrowsAsync<ObjectDisposedException>(() => connection.ConnectAsync("127.0.0.1", 1, "fixture"));
        await Assert.ThrowsAsync<ObjectDisposedException>(() => connection.RequestAsync(Json("""{"op":"ping"}""")));
        Assert.False(connection.Connected);
    }

    private static JsonObject Json(string value) => JsonNode.Parse(value)!.AsObject();
    private static JsonObject Error(string code) => new() { ["ok"] = false, ["error"] = code };
    private static JsonObject Sql(string text) => new() { ["op"] = "sql", ["sql"] = text };
    private static JsonObject Session(bool active, string? role = null) => new() { ["ok"] = true, ["results"] = new JsonArray(), ["in_transaction"] = active, ["read_role"] = role };
    private static Exchange Auth() => new("auth", Json("""{"ok":true,"protocol":1}"""));
    private static Exchange Probe(JsonObject response) => new("sql", response, "SHOW VIEWS;");
    private sealed record Exchange(string Operation, JsonObject? Response, string? Sql = null);

    private sealed class Peer : IAsyncDisposable
    {
        private readonly TcpListener listener = new(IPAddress.Loopback, 0);
        private readonly CancellationTokenSource deadline = new(TimeSpan.FromSeconds(15));
        public int Port { get; }
        public Task Completion { get; }
        public ConcurrentQueue<JsonObject> Requests { get; } = new();

        public Peer(params Exchange[] exchanges)
        {
            listener.Start();
            Port = ((IPEndPoint)listener.LocalEndpoint).Port;
            Completion = Task.Run(async () =>
            {
                using var socket = await listener.AcceptTcpClientAsync(deadline.Token);
                await using var stream = socket.GetStream();
                foreach (var exchange in exchanges)
                {
                    var header = new byte[4];
                    await stream.ReadExactlyAsync(header, deadline.Token);
                    var length = BinaryPrimitives.ReadInt32BigEndian(header);
                    Assert.InRange(length, 1, 1024 * 1024);
                    var body = new byte[length];
                    await stream.ReadExactlyAsync(body, deadline.Token);
                    var request = JsonNode.Parse(body)!.AsObject();
                    Requests.Enqueue(request);
                    Assert.Equal(exchange.Operation, request["op"]!.ToString());
                    if (exchange.Sql is not null) Assert.Equal(exchange.Sql, request["sql"]!.ToString());
                    if (exchange.Operation == "auth") Assert.Equal("fixture", request["token"]!.ToString());
                    if (exchange.Response is null) return;
                    body = JsonSerializer.SerializeToUtf8Bytes(exchange.Response);
                    BinaryPrimitives.WriteInt32BigEndian(header, body.Length);
                    await stream.WriteAsync(header, deadline.Token);
                    await stream.WriteAsync(body, deadline.Token);
                }
            });
        }

        public async ValueTask DisposeAsync()
        {
            await deadline.CancelAsync();
            listener.Stop();
            try { await Completion; }
            catch (OperationCanceledException) when (deadline.IsCancellationRequested) { }
            finally { deadline.Dispose(); }
        }
    }
}
