using System.Collections.Concurrent;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed record FolderTable(string Name, string Source, int Index, long? Rows, SqlTable Schema)
{
    public int Columns => Schema.Columns.Count;
}

internal sealed partial class CatlinkFolder : IDisposable
{
    private ArchiveTableReader? archive;
    private ArchiveCatalog? archiveCatalog;
    private readonly string directory;
    private readonly IReadOnlyList<FolderTable> baseTables;
    private readonly SemaphoreSlim commands = new(1);
    private readonly ConcurrentDictionary<string, SqlTableStatistics> sessionStatistics = new(StringComparer.OrdinalIgnoreCase);
    public IReadOnlyList<FolderTable> Tables => archiveCatalog is null ? baseTables : baseTables.Select(t => t with
        { Rows = archiveTransaction?.Count(t.Index) ?? archiveCatalog.Tables[t.Index].Rows }).ToArray();

    private CatlinkFolder(string directory, IEnumerable<FolderTable> tables, ArchiveTableReader? archive = null)
    {
        this.directory = directory;
        baseTables = tables.OrderBy(t => t.Name, StringComparer.OrdinalIgnoreCase).ToArray();
        this.archive = archive; archiveCatalog = archive?.Manifest;
    }

    public static async Task<CatlinkFolder> LoadAsync(string directory)
    {
        if (File.Exists(Path.Combine(directory, "library.ndb")))
        {
            var reader = ArchiveTableReader.TryOpen(directory);
            if (reader is not null)
                return new(directory, reader.Manifest.Tables.Select((t, i) => new FolderTable(t.Name, "", i, t.Rows, new SqlTable(t.Name, t.Columns.Select(ArchiveSqlColumn).ToList()))), reader);
        }
        var tables = new List<FolderTable>();
        foreach (var path in Directory.EnumerateFiles(directory, "*.ndb").Order(StringComparer.OrdinalIgnoreCase))
        {
            var schema = await ReadSqlCopyAsync(path, db => db.GetTablesAsync());
            tables.AddRange(schema.Select(t => new FolderTable(t.Name, path, -1, null, t)));
        }
        return new(directory, tables);
    }

    public string Label(FolderTable table) =>
        table.Source.Length > 0 && Tables.Count(t => t.Name.Equals(table.Name, StringComparison.OrdinalIgnoreCase)) > 1
            ? $"{table.Name} ({Path.GetFileName(table.Source)})" : table.Name;

    public SqlResult Summary() => new([L.Get("Table"), L.Get("Columns"), L.Get("Rows191")],
        Tables.Select(t => new[] { SqlValue.String(Label(t)), SqlValue.Number(t.Columns.ToString()),
            t.Rows.HasValue ? SqlValue.Number(t.Rows.Value.ToString()) : SqlValue.Null }).ToArray(), L.Get("TableList"));

    public async Task<LoadedQueryResult> LoadQueryAsync(SelectRows query)
    {
        await commands.WaitAsync();
        try { return await LoadedQueryResult.LoadAsync(SqlDatabase.ReadAllReadOnlyAsync(query, Schema, ScanAsync, name => FindTable(name).Rows)); }
        finally { commands.Release(); }
    }

    public async Task<SqlResult> ReadEditQueryAsync(string sql)
    {
        await commands.WaitAsync();
        try
        {
            if (SqlParser.Parse(sql) is not [SelectRows query]) throw new SqlException("編集用のSELECTを1文指定してください。");
            SqlResult header = new([], [], ""); var rows = new List<SqlValue[]>();
            await foreach (var batch in SqlDatabase.ReadAllReadOnlyAsync(query, Schema, ScanAsync, name => FindTable(name).Rows))
            { header = batch; rows.AddRange(batch.Rows); }
            return header with { Rows = rows.ToArray() };
        }
        finally { commands.Release(); }
    }

    public async Task<IReadOnlyList<SqlResult>> ExecuteAsync(string sql)
    {
        await commands.WaitAsync();
        try
        {
            var results = new List<SqlResult>();
            foreach (var part in SqlParser.ParseWithSource(sql))
            {
                var statement = part.Statement;
                switch (statement)
                {
                    case TransactionStatement transaction:
                        await ChangeTransactionAsync(transaction);
                        results.Add(new([], [], transaction.Operation.ToUpperInvariant() + " 完了")); break;
                    case ShowTables: results.Add(Summary()); break;
                    case AnalyzeTable analyze: results.Add(await AnalyzeAsync(FindTable(analyze.Name))); break;
                    case ExplainQuery explain:
                        results.Add(await SqlDatabase.ExplainReadOnlyAsync(explain, Schema, ScanAsync, name => FindTable(name).Rows)); break;
                    case SelectRows query:
                        results.Add(await SqlDatabase.QueryReadOnlyAsync(query, Schema, ScanAsync, name => FindTable(name).Rows)); break;
                    case InsertStatement insert:
                        results.Add(await WriteAsync(FindTable(insert.Table), statement, sql.Substring(part.Start, part.Length))); break;
                    case UpdateRows update:
                        results.Add(await WriteAsync(FindTable(update.Table), statement, sql.Substring(part.Start, part.Length))); break;
                    case DeleteRows delete:
                        results.Add(await WriteAsync(FindTable(delete.Table), statement, sql.Substring(part.Start, part.Length))); break;
                    default: throw new SqlException(L.Get("FolderSupportedSql"));
                }
            }
            if (results.Count == 0) throw new SqlException("SQL文を入力してください。");
            return results;
        }
        finally { commands.Release(); }
    }

    private SqlTable Schema(string name)
    {
        var table = FindTable(name);
        return sessionStatistics.TryGetValue(table.Name, out var statistics)
            ? table.Schema with { Statistics = statistics } : table.Schema;
    }

    private async Task<SqlResult> AnalyzeAsync(FolderTable table)
    {
        long sampled;
        SqlTableStatistics statistics;
        if (transactionActive)
        {
            long total = table.Rows ?? 0;
            if (table.Rows is null) await foreach (var row in ScanAsync(table.Name)) total++;
            sampled = SqlDatabase.ReadOnlySampleSize(table.Schema, total);
            statistics = await SqlDatabase.AnalyzeReadOnlySampleAsync(table.Schema, total, PendingSample());
            async IAsyncEnumerable<SqlValue[]> PendingSample()
            {
                long position = 0, selected = 0;
                await foreach (var row in ScanAsync(table.Name))
                {
                    if (selected >= sampled) yield break;
                    if (position++ < (long)(selected * (decimal)total / sampled)) continue;
                    selected++;
                    yield return table.Schema.Columns.Select(c => row.Values[c.Name]).ToArray();
                }
            }
        }
        else if (archive is not null)
        {
            long total = table.Rows!.Value;
            sampled = SqlDatabase.ReadOnlySampleSize(table.Schema, total);
            statistics = await SqlDatabase.AnalyzeReadOnlySampleAsync(table.Schema, total,
                ArchiveSampleAsync(archive, table.Index, total, (int)sampled));
        }
        else
        {
            statistics = await ReadSqlCopyAsync(table.Source, async db =>
            {
                await db.ExecuteAsync($"ANALYZE {SqlNames.Quote(table.Name)}");
                var analyzed = (await db.GetTablesAsync()).Single(t => t.Name.Equals(table.Name, StringComparison.OrdinalIgnoreCase));
                return analyzed.Statistics ?? throw new SqlException(L.Get("CouldNotGetStatisticsFor", table.Name));
            });
            sampled = statistics.RowCount;
        }
        sessionStatistics[table.Name] = statistics;
        var message = L.Get("ANALYZESampledNOfNRowsStatisticsAre", table.Name, statistics.RowCount, sampled);
        return new(["table_name", "total_rows", "sampled_rows"], [new[] { SqlValue.String(table.Name),
            SqlValue.Number(statistics.RowCount.ToString(System.Globalization.CultureInfo.InvariantCulture)),
            SqlValue.Number(sampled.ToString(System.Globalization.CultureInfo.InvariantCulture)) }], message)
            { ColumnTypes = ["TEXT", "INTEGER", "INTEGER"] };
    }

    private static async IAsyncEnumerable<SqlValue[]> ArchiveSampleAsync(ArchiveTableReader archive, int tableIndex,
        long totalRows, int sampleSize)
    {
        int pages = (sampleSize + ArchiveTableReader.PageSize - 1) / ArchiveTableReader.PageSize;
        for (int i = 0; i < pages; i++)
        {
            int begin = sampleSize * i / pages;
            int count = sampleSize * (i + 1) / pages - begin;
            long offset = begin + (long)((totalRows - sampleSize) * (i + 0.5m) / pages);
            long end = offset + count;
            while (offset < end)
            {
                var page = await archive.ReadPageAsync(tableIndex, offset, (int)(end - offset), preview: false).ConfigureAwait(false);
                if (page.NextOffset <= offset) throw new InvalidDataException(L.Get("ArchivePageLoadingMadeNoProgressWhileCollecting"));
                foreach (var row in page.Result.Rows) yield return row;
                offset = page.NextOffset;
            }
        }
    }

    private FolderTable FindTable(string name)
    {
        var matches = Tables.Where(t => t.Name.Equals(name, StringComparison.OrdinalIgnoreCase)).Take(2).ToArray();
        return matches.Length switch
        {
            1 => matches[0],
            0 => throw new SqlException(L.Get("TableNotFound", name)),
            _ => throw new SqlException(L.Get("MultipleDBsContainATableWithThisName", name))
        };
    }

    private static SqlColumn ArchiveSqlColumn(ArchiveColumn column)
    {
        var type = column.DeclaredType?.ToUpperInvariant() ?? "";
        var sqlType = type.Contains("INT") ? "INTEGER" :
            type.Contains("CHAR") || type.Contains("CLOB") || type.Contains("TEXT") ? "TEXT" :
            type.Contains("BLOB") || type.Length == 0 ? "BLOB" : "REAL";
        return new(column.Name, sqlType, false, false, false, null, null);
    }

    private async IAsyncEnumerable<SqlRow> ScanAsync(string name)
    {
        var table = FindTable(name);
        if (archiveCatalog is not null && archiveTransaction is { } pending)
        {
            await foreach (var row in pending.ScanAsync(table.Index, table.Schema)) yield return row;
            yield break;
        }
        if (archiveCatalog is not null) RequireArchive();
        long offset = 0;
        do
        {
            SqlResult result;
            if (archive is not null)
            {
                if (offset >= table.Rows) yield break;
                var page = await archive.ReadPageAsync(table.Index, offset, preview: false).ConfigureAwait(false);
                result = page.Result;
            }
            else
            {
                var sql = $"SELECT * FROM {SqlNames.Quote(table.Name)} LIMIT {SqlDatabase.MaxRows}";
                result = (await ReadSqlSourceAsync(table.Source, db => db.ExecuteAsync(sql))).Single();
            }
            foreach (var row in result.Rows)
            {
                yield return new(offset.ToString(), new(result.Columns.Zip(row, KeyValuePair.Create), StringComparer.OrdinalIgnoreCase));
                offset++;
            }
        } while (archive is not null && offset < table.Rows);
    }

    public async Task<(SqlResult Result, long? Next)> ReadPageAsync(FolderTable table, long offset)
    {
        if (archiveTransaction is { } pending)
        {
            var pendingRows = new List<SqlValue[]>(); long position = 0;
            await foreach (var row in pending.ScanAsync(table.Index, table.Schema))
            {
                if (position++ < offset) continue;
                pendingRows.Add(table.Schema.Columns.Select(c => row.Values[c.Name]).ToArray());
                if (pendingRows.Count == 100) break;
            }
            long count = pending.Count(table.Index);
            return (new(table.Schema.Columns.Select(c => c.Name).ToArray(), pendingRows, L.Get("RowsNTotal", table.Name, pendingRows.Count, count)), offset + pendingRows.Count < count ? offset + pendingRows.Count : null);
        }
        if (archiveCatalog is not null)
        {
            var page = await RequireArchive().ReadPageAsync(table.Index, offset);
            return (page.Result, page.NextOffset < page.TotalRows ? page.NextOffset : null);
        }
        var rows = await ReadSqlSourceAsync(table.Source, db => db.ExecuteAsync($"SELECT * FROM {SqlNames.Quote(table.Name)} LIMIT 101 OFFSET {offset};"));
        var result = rows.Single();
        return (result with { Rows = result.Rows.Take(100).ToArray() }, result.Rows.Count > 100 && offset + 100 <= 4096 ? offset + 100 : null);
    }

    private static async Task<T> ReadSqlCopyAsync<T>(string path, Func<SilverHamuSqlConnection, Task<T>> read)
    {
        if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new InvalidDataException(L.Get("LinkedDatabasesCannotBeLoaded"));
        var temp = Directory.CreateTempSubdirectory("silverhamu-folder-");
        try
        {
            var copy = Path.Combine(temp.FullName, "copy.ndb");
            using (var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
            using (var output = File.Create(copy)) await input.CopyToAsync(output);
            using var db = await SilverHamuSqlConnection.OpenAsync(copy);
            return await read(db);
        }
        finally { temp.Delete(recursive: true); }
    }

    public void Dispose()
    {
        writeConnection?.Dispose(); writeConnection = null;
        archiveTransaction = null; transactionActive = false;
        archive?.Dispose(); archive = null;
    }
}
