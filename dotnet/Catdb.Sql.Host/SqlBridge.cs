using System.Text.Json;
using System.Text.Json.Nodes;

namespace Catdb.Sql.Host;

// The service owns the native transaction and reports its state after every KV operation.
internal sealed class SqlBridge(Stream input, Stream output)
{
    private const int MaxFrameBytes = 8 * 1024 * 1024;
    private bool inTransaction;
    private bool transportFailed;
    private string? readRole;

    public async Task RunAsync()
    {
        var database = new SqlDatabase(SendAsync, () => inTransaction);
        while (ReadFrame() is { } command)
        {
            var sql = command["sql"]?.GetValue<string>() ?? throw new InvalidDataException("Missing SQL.");
            inTransaction = ReadTransactionState(command);
            readRole = command.ContainsKey("role")
                ? command["role"]?.GetValue<string>() ?? throw new InvalidDataException("Missing read role.")
                : null;
            IReadOnlyList<SqlResult> results;
            try
            {
                results = readRole is not null
                    ? [await database.QueryRoleViewAsync(sql, readRole)]
                    : await database.ExecuteAsync(sql);
            }
            catch (Exception error) when (!transportFailed)
            {
                WriteFrame(new { error = error.Message, in_transaction = inTransaction, read_role = readRole });
                continue;
            }
            try { WriteFrame(new { results, in_transaction = inTransaction, read_role = readRole }); }
            catch (FrameSizeException)
            {
                WriteFrame(new { error = "SQL result exceeds the response limit. Executed statements may have taken effect; check transaction state before retrying.", in_transaction = inTransaction, read_role = readRole });
            }
        }
    }

    private Task<JsonObject> SendAsync(JsonObject request)
    {
        if (transportFailed) throw new IOException("Service connection lost.");
        JsonObject response;
        try
        {
            WriteFrame(new { request, read_role = readRole });
            var envelope = ReadFrame() ?? throw new EndOfStreamException("Missing KV response.");
            inTransaction = ReadTransactionState(envelope);
            response = envelope["response"] as JsonObject ?? throw new InvalidDataException("Missing KV response.");
            if (response["ok"]?.GetValue<bool>() == true) return Task.FromResult(response);
            if (response["ok"] is null) throw new InvalidDataException("Missing KV response status.");
        }
        catch
        {
            // An incomplete exchange cannot safely carry rollback or a subsequent command.
            transportFailed = true;
            throw;
        }
        throw new SqlException(response["error"]?.GetValue<string>() ?? "KV operation failed.");
    }

    private static bool ReadTransactionState(JsonObject envelope) =>
        envelope["in_transaction"]?.GetValue<bool>() ?? throw new InvalidDataException("Missing transaction state.");

    private JsonObject? ReadFrame()
    {
        using var buffer = new MemoryStream();
        for (int next; (next = input.ReadByte()) >= 0;)
        {
            if (next == '\n')
                return JsonNode.Parse(buffer.GetBuffer().AsSpan(0, (int)buffer.Length)) as JsonObject
                    ?? throw new InvalidDataException("Expected a JSON object.");
            if (buffer.Length >= MaxFrameBytes) throw new InvalidDataException("SQL request frame exceeds 8 MiB.");
            buffer.WriteByte((byte)next);
        }
        if (buffer.Length != 0) throw new EndOfStreamException("Incomplete SQL request.");
        return null;
    }

    private void WriteFrame<T>(T frame)
    {
        // Stage only complete, bounded messages before making them visible on the pipe.
        using var buffer = new FrameBuffer();
        JsonSerializer.Serialize(buffer, frame);
        buffer.WriteTo(output);
        output.WriteByte((byte)'\n');
        output.Flush();
    }

    private sealed class FrameSizeException : IOException { }

    private sealed class FrameBuffer : MemoryStream
    {
        public override void Write(byte[] buffer, int offset, int count)
        {
            if (Length + count > MaxFrameBytes) throw new FrameSizeException();
            base.Write(buffer, offset, count);
        }

        public override void Write(ReadOnlySpan<byte> buffer)
        {
            if (Length + buffer.Length > MaxFrameBytes) throw new FrameSizeException();
            base.Write(buffer);
        }
    }
}
