using System.Globalization;
using System.Text.Json.Nodes;
using NewDbms;

namespace Catdb.Sql;

public sealed class CatdbRequestException(string message) : Exception(message);

public sealed class EmbeddedCatdbStore : IDisposable
{
    private readonly object gate = new();
    private readonly NewDbmsDatabase database;
    private NewDbmsTransaction? transaction;
    private bool disposed;
    public string DatabasePath { get; }
    public bool InTransaction { get { lock (gate) return transaction is not null; } }

    private EmbeddedCatdbStore(NewDbmsDatabase database, string path) { this.database = database; DatabasePath = path; }

    public static Task<EmbeddedCatdbStore> OpenAsync(string path, bool create = false) => Task.Run(() =>
    {
        if (string.IsNullOrWhiteSpace(path) || !Path.IsPathFullyQualified(path)) throw new ArgumentException("DBファイルのフルパスを指定してください。");
        path = Path.GetFullPath(path);
        if (create && File.Exists(path)) throw new IOException("既存のDBファイルには上書きできません。");
        if (!create && !File.Exists(path)) throw new FileNotFoundException("DBファイルが見つかりません。", path);
        if (!Directory.Exists(Path.GetDirectoryName(path))) throw new DirectoryNotFoundException("DBの保存先フォルダーが見つかりません。");
        try
        {
            var limits = new NewDbmsResourceLimits();
            var database = create ? NewDbmsDatabase.Create(path, Guid.NewGuid(), limits) : NewDbmsDatabase.Open(path, limits);
            return new EmbeddedCatdbStore(database, path);
        }
        catch (NewDbmsException error) when (error.StatusCode == 23)
        {
            throw new IOException("このDBは他の接続またはプロセスが使用中です。使用中のDBは直接開けません。", error);
        }
        catch (NewDbmsException error)
        {
            throw new IOException($"catdbを開けません（status {error.StatusCode}）。トランザクション形式のcatdbか、アクセス権とファイルの状態を確認してください。ファイルの作り直しは行いません。", error);
        }
        catch (Exception error) when (error is DllNotFoundException or BadImageFormatException or EntryPointNotFoundException)
        {
            throw new IOException("現在のOS・アーキテクチャに対応する catdb ネイティブライブラリを配置してください。", error);
        }
    });

    public Task<JsonObject> RequestAsync(JsonObject request)
    {
        var snapshot = (JsonObject)request.DeepClone();
        return Task.Run(() =>
        {
            lock (gate)
            {
                ObjectDisposedException.ThrowIf(disposed, this);
                try { return Request(snapshot); }
                catch (NewDbmsConflictException) { throw new CatdbRequestException("conflict"); }
                catch (NewDbmsReopenRequiredException error)
                { throw new CatdbRequestException(error.StatusCode switch { 20 => "commit_uncertain", 25 => "checkpoint_uncertain", _ => "reopen_required" }); }
                catch (NewDbmsResourceLimitException) { throw new CatdbRequestException("resource_exhausted"); }
                catch (NewDbmsException error)
                { throw new CatdbRequestException($"catdb操作に失敗しました（status {error.StatusCode}）。"); }
            }
        });
    }

    private JsonObject Request(JsonObject request)
    {
        static string Decimal(ulong value) => value.ToString(CultureInfo.InvariantCulture);
        static byte[] Bytes(JsonNode? value) => Convert.FromHexString(value?.ToString() ?? "");
        var response = new JsonObject { ["ok"] = true };
        switch (request["op"]?.ToString())
        {
            case "begin":
                if (transaction is not null) throw new CatdbRequestException("トランザクションは既に開始されています。");
                transaction = database.BeginTransaction(); response["version"] = Decimal(transaction.SnapshotVersion); break;
            case "commit":
                var pending = RequireTransaction(); transaction = null;
                using (pending) response["version"] = Decimal(pending.Commit());
                break;
            case "rollback": RequireTransaction().Dispose(); transaction = null; break;
            case "lock_keys": RequireTransaction().LockKeys(request["keys"]!.AsArray().Select(Bytes).ToArray()); break;
            case "savepoint": RequireTransaction().CreateSavepoint(request["name"]!.GetValue<string>()); break;
            case "rollback_to": RequireTransaction().RollbackToSavepoint(request["name"]!.GetValue<string>()); break;
            case "release_savepoint": RequireTransaction().ReleaseSavepoint(request["name"]!.GetValue<string>()); break;
            case "checkpoint":
                if (transaction is not null) throw new CatdbRequestException("チェックポイントの前にトランザクションを終了してください。");
                var checkpoint = database.Checkpoint();
                response["version"] = Decimal(checkpoint.Version);
                response["records"] = checkpoint.Records;
                break;
            case "get":
                var value = RequireTransaction().Get(Bytes(request["key"]));
                response["value"] = value is null ? null : Convert.ToHexString(value); break;
            case "put": RequireTransaction().Put(Bytes(request["key"]), Bytes(request["value"])); break;
            case "delete": RequireTransaction().Delete(Bytes(request["key"])); break;
            case "scan":
                var limits = new NewDbmsScanLimits(request["rows"]?.GetValue<int>() ?? 100, request["bytes"]?.GetValue<int>() ?? 65536, request["visits"]?.GetValue<int>() ?? 4096);
                var page = RequireTransaction().ScanPage(Bytes(request["start"]), request["end"] is null ? null : Bytes(request["end"]), request["after"] is null ? null : Bytes(request["after"]), limits);
                response["rows"] = new JsonArray(page.Entries.Select(x => (JsonNode)new JsonArray(Convert.ToHexString(x.Key), Convert.ToHexString(x.Value))).ToArray());
                response["next_after"] = page.NextAfter is null ? null : Convert.ToHexString(page.NextAfter);
                response["visited"] = page.VisitedEntries; break;
            default: throw new CatdbRequestException("未対応の組み込み操作です。");
        }
        return response;
    }

    public Task BackupAsync(string destination) => Task.Run(() =>
    {
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            database.Backup(destination);
        }
    });

    public static Task RestoreAsync(string backupPath, string destination) =>
        Task.Run(() => { NewDbmsDatabase.Restore(backupPath, destination); });

    public Task<NewDbmsExport> ExportSnapshotAsync(string destination, Guid generation, byte[] start, byte[]? end)
    {
        var path = Path.GetFullPath(destination);
        var first = start.ToArray();
        var last = end?.ToArray();
        return Task.Run(() =>
        {
            lock (gate)
            {
                ObjectDisposedException.ThrowIf(disposed, this);
                var temp = Path.Combine(Path.GetDirectoryName(path)!, $".{Guid.NewGuid():N}.snapshot.tmp");
                try
                {
                    var info = database.ExportValues(temp, generation, first, last);
                    File.Move(temp, path, overwrite: false);
                    return info;
                }
                finally { File.Delete(temp); }
            }
        });
    }

    private NewDbmsTransaction RequireTransaction() => transaction ?? throw new CatdbRequestException("トランザクションがありません。");
    public void Dispose()
    {
        lock (gate)
        {
            if (disposed) return;
            transaction?.Dispose(); transaction = null;
            database.Dispose(); disposed = true;
        }
    }
}
