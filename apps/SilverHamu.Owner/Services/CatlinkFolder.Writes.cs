using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed partial class CatlinkFolder
{
    private bool transactionActive;
    private SilverHamuSqlConnection? writeConnection;
    private string? writeSource;
    private ArchiveFolderTransaction? archiveTransaction;
    private Exception? archiveReadFailure;
    private readonly List<string> savepoints = [];
    public bool InTransaction => transactionActive;

    private ArchiveTableReader RequireArchive() => archiveReadFailure is { } error
        ? throw new IOException(L.Get("FolderReopenAfterCommit"), error)
        : archive ?? throw new ObjectDisposedException(nameof(CatlinkFolder));

    private Task<T> ReadSqlSourceAsync<T>(string path, Func<SilverHamuSqlConnection, Task<T>> read) =>
        writeConnection is not null && string.Equals(path, writeSource, StringComparison.OrdinalIgnoreCase)
            ? read(writeConnection) : ReadSqlCopyAsync(path, read);

    private async Task ChangeTransactionAsync(TransactionStatement statement)
    {
        if (statement.Savepoint is { } name)
        {
            if (!transactionActive) throw new SqlException("SAVEPOINTにはBEGINが必要です。");
            int index = savepoints.FindLastIndex(s => s.Equals(name, StringComparison.OrdinalIgnoreCase));
            if (statement.Operation != "savepoint" && index < 0) throw new SqlException("SAVEPOINTが存在しません。");
            if (statement.Operation == "savepoint" && savepoints.Count >= 128) throw new SqlException("SAVEPOINTは128個までです。");
            if (archiveTransaction is not null) archiveTransaction.Savepoint(statement.Operation, name);
            else if (writeConnection is not null) await writeConnection.ExecuteAsync(SavepointSql(statement.Operation, name));
            if (statement.Operation == "savepoint") savepoints.Add(name);
            else { if (statement.Operation == "rollback_to") index++; savepoints.RemoveRange(index, savepoints.Count - index); }
            sessionStatistics.Clear(); return;
        }
        switch (statement.Operation)
        {
            case "begin":
                if (transactionActive) throw new SqlException("トランザクションは既に開始されています。");
                if (archiveCatalog is not null) archiveTransaction = new(RequireArchive());
                transactionActive = true; savepoints.Clear(); break;
            case "rollback":
                if (!transactionActive) throw new SqlException("トランザクションがありません。");
                try { if (writeConnection is not null) await writeConnection.ExecuteAsync("ROLLBACK;"); }
                finally { EndTransaction(); }
                break;
            case "commit":
                if (!transactionActive) throw new SqlException("トランザクションがありません。");
                if (archiveTransaction is { } pending)
                {
                    ArchiveCatalog catalog;
                    try { catalog = await pending.CommitAsync(); }
                    catch (SilverHamuRequestException error) when (error.Message == "commit_uncertain") { EndTransaction(); archiveReadFailure = error; throw; }
                    EndTransaction(); archiveCatalog = catalog;
                    var previous = archive;
                    try { archive = ArchiveTableReader.TryOpen(directory) ?? throw new IOException("書庫の目録が見つかりません。"); }
                    catch (Exception error) { archive = null; archiveReadFailure = error; }
                    finally
                    {
                        // 確定後の一時コピー削除失敗で更新失敗と通知すると、同じSQLの再実行を招く。
                        try { previous?.Dispose(); }
                        catch (IOException) { }
                        catch (UnauthorizedAccessException) { }
                    }
                }
                else
                {
                    try { if (writeConnection is not null) await writeConnection.ExecuteAsync("COMMIT;"); }
                    finally { EndTransaction(); }
                }
                break;
            default: throw new SqlException("未対応のトランザクション操作です。");
        }
        sessionStatistics.Clear();
    }

    private static string SavepointSql(string operation, string name) => (operation switch
    {
        "savepoint" => "SAVEPOINT ", "rollback_to" => "ROLLBACK TO SAVEPOINT ", "release_savepoint" => "RELEASE SAVEPOINT ",
        _ => throw new SqlException("未対応のSAVEPOINT操作です。")
    }) + SqlNames.Quote(name) + ";";

    private void EndTransaction()
    {
        writeConnection?.Dispose(); writeConnection = null; writeSource = null;
        archiveTransaction = null; transactionActive = false; savepoints.Clear();
    }

    private async Task<SqlResult> WriteAsync(FolderTable table, SqlStatement statement, string sql)
    {
        bool own = !transactionActive;
        if (own) await ChangeTransactionAsync(new("begin"));
        try
        {
            SqlResult result;
            if (archiveTransaction is not null) result = await archiveTransaction.ExecuteAsync(table.Index, table.Schema, statement);
            else
            {
                if (writeConnection is not null && !string.Equals(writeSource, table.Source, StringComparison.OrdinalIgnoreCase))
                    throw new SqlException(L.Get("FolderOneDatabasePerTransaction"));
                if (writeConnection is null)
                {
                    if ((File.GetAttributes(table.Source) & FileAttributes.ReparsePoint) != 0) throw new IOException(L.Get("LinkedDatabasesCannotBeLoaded"));
                    var connection = await SilverHamuSqlConnection.OpenAsync(table.Source);
                    try
                    {
                        await connection.ExecuteAsync("BEGIN;");
                        foreach (string name in savepoints) await connection.ExecuteAsync(SavepointSql("savepoint", name));
                        writeConnection = connection; writeSource = table.Source;
                    }
                    catch { connection.Dispose(); throw; }
                }
                result = (await writeConnection.ExecuteAsync(sql)).Single();
            }
            sessionStatistics.Clear();
            if (own) await ChangeTransactionAsync(new("commit"));
            else result = result with { Message = result.Message + " 未確定です。COMMITで確定、ROLLBACKで取消。" };
            return result;
        }
        catch
        {
            if (own && transactionActive) await ChangeTransactionAsync(new("rollback"));
            throw;
        }
    }
}
