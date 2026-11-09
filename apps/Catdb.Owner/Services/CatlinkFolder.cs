using Catdb.Sql;

namespace Catdb.Owner.Services;

internal sealed record FolderTable(string Name, string Source, int Index, long? Rows, SqlTable Schema)
{
    public int Columns => Schema.Columns.Count;
}

internal sealed class CatlinkFolder : IDisposable
{
    private readonly ArchiveTableReader? archive;
    public IReadOnlyList<FolderTable> Tables { get; }

    private CatlinkFolder(IEnumerable<FolderTable> tables, ArchiveTableReader? archive = null)
    {
        Tables = tables.OrderBy(t => t.Name, StringComparer.OrdinalIgnoreCase).ToArray();
        this.archive = archive;
    }

    public static async Task<CatlinkFolder> LoadAsync(string directory)
    {
        if (File.Exists(Path.Combine(directory, "library.ndb")))
        {
            var reader = ArchiveTableReader.TryOpen(directory);
            if (reader is not null)
                return new(reader.Manifest.Tables.Select((t, i) => new FolderTable(t.Name, "", i, t.Rows, new SqlTable(t.Name, t.Columns.Select(ArchiveSqlColumn).ToList()))), reader);
        }
        var tables = new List<FolderTable>();
        foreach (var path in Directory.EnumerateFiles(directory, "*.ndb").Order(StringComparer.OrdinalIgnoreCase))
        {
            var schema = await ReadSqlCopyAsync(path, db => db.GetTablesAsync());
            tables.AddRange(schema.Select(t => new FolderTable(t.Name, path, -1, null, t)));
        }
        return new(tables);
    }

    public string Label(FolderTable table) =>
        table.Source.Length > 0 && Tables.Count(t => t.Name.Equals(table.Name, StringComparison.OrdinalIgnoreCase)) > 1
            ? $"{table.Name} ({Path.GetFileName(table.Source)})" : table.Name;

    public SqlResult Summary() => new(["テーブル", "列数", "行数"],
        Tables.Select(t => new[] { SqlValue.String(Label(t)), SqlValue.Number(t.Columns.ToString()),
            t.Rows.HasValue ? SqlValue.Number(t.Rows.Value.ToString()) : SqlValue.Null }).ToArray(), "テーブル一覧");

    public async Task<IReadOnlyList<SqlResult>> ExecuteAsync(string sql)
    {
        var results = new List<SqlResult>();
        foreach (var statement in SqlParser.Parse(sql))
        {
            if (statement is ShowTables)
            {
                results.Add(Summary());
                continue;
            }
            if (statement is not SelectRows query)
                throw new SqlException("フォルダー閲覧では SELECT と SHOW TABLES を実行できます。");
            results.Add(await SqlDatabase.QueryReadOnlyAsync(query, name => FindTable(name).Schema, ScanAsync, name => FindTable(name).Rows));
        }
        return results;
    }

    private FolderTable FindTable(string name)
    {
        var matches = Tables.Where(t => t.Name.Equals(name, StringComparison.OrdinalIgnoreCase)).Take(2).ToArray();
        return matches.Length switch
        {
            1 => matches[0],
            0 => throw new SqlException($"テーブルが見つかりません: {name}"),
            _ => throw new SqlException($"複数のDBに同名のテーブルがあります。対象のDBを直接開いてください: {name}")
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
                result = (await ReadSqlCopyAsync(table.Source, db => db.ExecuteAsync(sql))).Single();
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
        if (archive is not null)
        {
            var page = await archive.ReadPageAsync(table.Index, offset);
            return (page.Result, page.NextOffset < page.TotalRows ? page.NextOffset : null);
        }
        var rows = await ReadSqlCopyAsync(table.Source, db => db.ExecuteAsync($"SELECT * FROM {SqlNames.Quote(table.Name)} LIMIT 101 OFFSET {offset};"));
        var result = rows.Single();
        return (result with { Rows = result.Rows.Take(100).ToArray() }, result.Rows.Count > 100 && offset + 100 <= 4096 ? offset + 100 : null);
    }

    private static async Task<T> ReadSqlCopyAsync<T>(string path, Func<CatdbSqlConnection, Task<T>> read)
    {
        if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new InvalidDataException("リンクされたデータベースは読み込めません。");
        var temp = Directory.CreateTempSubdirectory("catdb-folder-");
        try
        {
            var copy = Path.Combine(temp.FullName, "copy.ndb");
            using (var input = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
            using (var output = File.Create(copy)) await input.CopyToAsync(output);
            using var db = await CatdbSqlConnection.OpenAsync(copy);
            return await read(db);
        }
        finally { temp.Delete(recursive: true); }
    }

    public void Dispose() => archive?.Dispose();
}
