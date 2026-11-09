namespace Catdb.Sql;

/// <summary>A SQL session on a local catdb transactional database.</summary>
public sealed class CatdbSqlConnection : IDisposable
{
    private readonly EmbeddedCatdbStore store;
    private readonly SqlDatabase sql;
    private readonly SemaphoreSlim commands = new(1);

    private CatdbSqlConnection(EmbeddedCatdbStore store)
    {
        this.store = store;
        sql = new(store.RequestAsync, () => store.InTransaction);
    }

    public string DatabasePath => store.DatabasePath;
    public bool InTransaction => store.InTransaction;

    /// <summary>Opens an existing database, or creates a new file exclusively when create is true.</summary>
    public static async Task<CatdbSqlConnection> OpenAsync(string path, bool create = false)
    {
        var store = await EmbeddedCatdbStore.OpenAsync(Path.GetFullPath(path), create).ConfigureAwait(false);
        return new CatdbSqlConnection(store);
    }

    public Task<IReadOnlyList<SqlResult>> ExecuteAsync(string statement) =>
        RunAsync(() => sql.ExecuteAsync(statement));

    public Task<IReadOnlyList<SqlTable>> GetTablesAsync() => RunAsync(sql.GetTablesAsync);
    public Task<IReadOnlyList<SqlTable>> GetViewsAsync() => RunAsync(sql.GetViewsAsync);

    /// <summary>Saves the committed database state to a new backup file.</summary>
    public Task BackupAsync(string destination) => store.BackupAsync(destination);

    /// <summary>Validates a backup and restores it to a new database path.</summary>
    public static Task RestoreAsync(string backupPath, string destination) =>
        EmbeddedCatdbStore.RestoreAsync(backupPath, destination);

    public Task<SqlResult> QueryPublishedViewAsync(string statement, IReadOnlyCollection<string> allowedViews) =>
        RunAsync(() => sql.QueryPublishedViewAsync(statement, allowedViews));

    public Task<SqlResult> QueryRoleViewAsync(string statement, string role) =>
        RunAsync(() => sql.QueryRoleViewAsync(statement, role));

    public Task<IReadOnlyList<string>> ListRoleViewsAsync(string role) =>
        RunAsync(() => sql.ListRoleViewsAsync(role));

    private async Task<T> RunAsync<T>(Func<Task<T>> command)
    {
        await commands.WaitAsync().ConfigureAwait(false);
        try { return await command().ConfigureAwait(false); }
        finally { commands.Release(); }
    }

    /// <summary>Closes the native database and releases any uncommitted transaction.</summary>
    public void Dispose() => store.Dispose();
}
