using System.Text.Json;
using System.Text.Json.Nodes;
using SilverHamu.Sql;
using ServiceClient = global::SilverHamu.SilverHamuClient;
using ServerException = global::SilverHamu.SilverHamuServerException;

namespace SilverHamu.Owner.Services;

public sealed class ServiceConnection : IDisposable
{
    private readonly object gate = new();
    private ServiceClient? client;
    private bool disposed;

    public bool Connected => client is not null && !disposed;
    public bool SupportsSql { get; private set; }
    public string? ReadRole { get; private set; }
    public bool SqlTransactionActive { get; private set; }

    public Task ConnectAsync(string host, int port, string token) => Task.Run(() =>
    {
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            if (client is not null) throw new InvalidOperationException(L.Get("AlreadyConnected"));
            ServiceClient? candidate = null;
            try
            {
                candidate = ServiceClient.ConnectToken(host, port, token);
                try { ReadSession(candidate); }
                catch (ServerException error) when (error.Code == "sql_unavailable")
                {
                    SupportsSql = false;
                    ReadRole = null;
                    SqlTransactionActive = false;
                }
                client = candidate;
            }
            catch (ServerException error)
            {
                candidate?.Dispose();
                throw new SilverHamuRequestException(error.Code);
            }
            catch
            {
                candidate?.Dispose();
                throw;
            }
        }
    });

    public Task<JsonObject> RequestAsync(JsonObject request) => Task.Run(() =>
    {
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            var connection = client ?? throw new InvalidOperationException(L.Get("ConnectToAService"));
            var operation = request["op"]?.ToString();
            try
            {
                var response = ReadObject(connection.Request(request));
                if (operation == "sql") ReadTransactionState(response);
                return response;
            }
            catch (ServerException error)
            {
                if (RequiresReconnect(error.Code)) CloseConnection();
                else if (operation == "sql" && SupportsSql)
                {
                    // SDK 0.2.0 exposes only the error code, so a failed response cannot supply transaction state.
                    try { ReadSession(connection); }
                    catch (Exception stateError)
                    {
                        CloseConnection();
                        throw new IOException(L.Get("CouldNotDetermineTheSQLTransactionStateReconnect"), stateError);
                    }
                }
                throw new SilverHamuRequestException(error.Code);
            }
            catch (ArgumentException error) when (error.ParamName == "request")
            {
                throw new InvalidOperationException(L.Get("RequestsMustBeMiBOrSmaller"), error);
            }
            catch (Exception error)
            {
                CloseConnection();
                throw new IOException(operation is "commit" or "sql"
                    ? L.Get("TheCommitOutcomeIsUnknownDoNotRetry")
                    : L.Get("TheConnectionWasInterruptedReconnect"), error);
            }
        }
    });

    private void ReadSession(ServiceClient connection)
    {
        // SDK 0.2.0 does not retain authentication metadata; this read-only query also works for view roles.
        var response = ReadObject(connection.Request(new { op = "sql", sql = "SHOW VIEWS;" }));
        ReadTransactionState(response);
        ReadRole = response["read_role"]?.GetValue<string>();
        SupportsSql = true;
    }

    private void ReadTransactionState(JsonObject response) =>
        SqlTransactionActive = response["in_transaction"]?.GetValue<bool>()
            ?? throw new IOException(L.Get("TheTransactionStateIsUnknown"));

    private static JsonObject ReadObject(JsonElement response) =>
        JsonNode.Parse(response.GetRawText()) as JsonObject ?? throw new IOException(L.Get("TheResponseIsInvalid"));

    private static bool RequiresReconnect(string code) => code is
        "commit_uncertain" or "checkpoint_uncertain" or "reopen_required" or "unauthorized" or "sql_host_failed_outcome_unknown";

    private void CloseConnection()
    {
        client?.Dispose();
        client = null;
        SupportsSql = false;
        ReadRole = null;
        SqlTransactionActive = false;
    }

    public void Dispose()
    {
        lock (gate)
        {
            if (disposed) return;
            disposed = true;
            CloseConnection();
        }
    }
}
