using System.Globalization;
using System.Text.Json;
using NewDbms;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed class ArchiveFolderTransaction(ArchiveTableReader reader)
{
    private Dictionary<(int Table, long Row), byte[]?> changes = [];
    private Dictionary<int, long> nextRows = [];
    private long stagedBytes;
    private sealed record State(Dictionary<(int Table, long Row), byte[]?> Changes, Dictionary<int, long> NextRows, long Bytes);
    private readonly List<(string Name, State State)> savepoints = [];
    private State Capture() => new(new(changes), new(nextRows), stagedBytes);
    private void Restore(State state) { changes = new(state.Changes); nextRows = new(state.NextRows); stagedBytes = state.Bytes; }
    private long Next(int table) => nextRows.GetValueOrDefault(table, reader.Manifest.Tables[table].Rows);
    public long Count(int table) => reader.Manifest.Tables[table].Rows + changes.Count(p => p.Key.Table == table && p.Key.Row >= reader.Manifest.Tables[table].Rows && p.Value is not null)
        - changes.Count(p => p.Key.Table == table && p.Key.Row < reader.Manifest.Tables[table].Rows && p.Value is null);

    public void Savepoint(string operation, string name)
    {
        if (operation == "savepoint")
        {
            if (savepoints.Count >= 128) throw new SqlException("SAVEPOINTは128個までです。");
            savepoints.Add((name, Capture())); return;
        }
        int index = savepoints.FindLastIndex(s => s.Name.Equals(name, StringComparison.OrdinalIgnoreCase));
        if (index < 0) throw new SqlException("SAVEPOINTが存在しません。");
        if (operation == "rollback_to") { Restore(savepoints[index].State); index++; }
        savepoints.RemoveRange(index, savepoints.Count - index);
    }

    public async IAsyncEnumerable<SqlRow> ScanAsync(int table, SqlTable schema)
    {
        long position = 0, total = reader.Manifest.Tables[table].Rows;
        while (position < total)
        {
            var page = await reader.ReadRawPageAsync(table, position);
            if (page.Count == 0) throw new InvalidDataException("書庫の行読込が進みませんでした。");
            foreach (var raw in page)
            {
                long index = position++;
                byte[]? encoded = changes.TryGetValue((table, index), out var changed) ? changed : raw;
                if (encoded is not null) yield return Row(index, encoded);
            }
        }
        long end = Next(table);
        for (position = total; position < end; position++)
            if (changes.GetValueOrDefault((table, position)) is { } encoded) yield return Row(position, encoded);

        SqlRow Row(long index, byte[] encoded)
        {
            var values = reader.DecodeValues(encoded, schema.Columns.Count);
            return new(index.ToString(CultureInfo.InvariantCulture), schema.Columns.Select((column, i) => (column.Name, Value: values[i]))
                .ToDictionary(p => p.Name, p => p.Value, StringComparer.OrdinalIgnoreCase));
        }
    }

    public async Task<SqlResult> ExecuteAsync(int table, SqlTable schema, SqlStatement statement)
    {
        var before = Capture();
        try { return await SqlDatabase.ExecuteExternalWriteAsync(schema, statement, ScanAsync(table, schema), Stage); }
        catch { Restore(before); throw; }

        async Task Stage(SqlRow? oldRow, SqlRow? newRow)
        {
            long index;
            byte[]? original = null;
            if (oldRow is null) { index = Next(table); nextRows[table] = checked(index + 1); }
            else
            {
                index = long.Parse(oldRow.StorageKey, CultureInfo.InvariantCulture);
                if (newRow is not null)
                    original = changes.TryGetValue((table, index), out var encoded) ? encoded : (await reader.ReadRawPageAsync(table, index, 1)).Single();
            }
            byte[]? value = newRow is null ? null : reader.EncodeValues(schema.Columns.Select(c => newRow.Values[c.Name]).ToArray(), original);
            stagedBytes += (value?.LongLength ?? 0) - (changes.TryGetValue((table, index), out var previous) ? previous?.LongLength ?? 0 : -64);
            if (stagedBytes > 128L * 1024 * 1024) throw new SqlException("書庫の未確定変更が128 MiBを超えました。更新を分割してください。");
            changes[(table, index)] = value;
        }
    }

    public async Task<ArchiveCatalog> CommitAsync()
    {
        if (changes.Count == 0) return reader.Manifest;
        var files = new List<string>();
        bool publishing = false;
        try
        {
            using var library = NewDbmsDatabase.Open(Path.Combine(reader.ArchiveDirectory, "library.ndb"));
            using var transaction = library.BeginTransaction();
            var current = transaction.Get("palv/import/manifest/v1"u8.ToArray());
            if (current is null || !current.AsSpan().SequenceEqual(reader.ManifestBytes)) throw new IOException("書庫の目録が別の接続で変更されました。再接続してください。");
            var tables = (ArchiveTable[])reader.Manifest.Tables.Clone();
            foreach (int table in changes.Keys.Select(k => k.Table).Distinct().Order())
                tables[table] = await WriteTableAsync(table, files);
            var catalog = reader.Manifest with { Tables = tables };
            var bytes = JsonSerializer.SerializeToUtf8Bytes(catalog);
            if (bytes.Length > 8 * 1024 * 1024) throw new IOException("書庫の目録が8 MiBを超えます。");
            transaction.Put("palv/import/manifest/v1"u8.ToArray(), bytes);
            publishing = true;
            transaction.Commit();
            return catalog;
        }
        catch (NewDbmsReopenRequiredException error) when (publishing && error.OutcomeUncertain)
        { throw new SilverHamuRequestException("commit_uncertain"); }
        finally
        {
            // 目録COMMITの成否が不明な場合、新セグメントを消すと確定済みデータを失う可能性がある。
            if (!publishing)
                foreach (string path in files) { File.Delete(path); File.Delete(path + ".owner"); }
        }
    }

    private async Task<ArchiveTable> WriteTableAsync(int index, List<string> files)
    {
        var table = reader.Manifest.Tables[index];
        var segments = new List<ArchiveSegment>();
        bool deleted = changes.Any(p => p.Key.Table == index && p.Key.Row < table.Rows && p.Value is null);
        if (deleted)
        {
            using var writer = new SegmentWriter(reader.ArchiveDirectory, index, 0, files, segments);
            await CopyRange(0, table.Rows, writer);
            AppendRows(writer);
        }
        else
        {
            foreach (var segment in table.Segments)
            {
                bool updated = changes.Keys.Any(k => k.Table == index && k.Row >= segment.Start && k.Row < segment.Start + segment.Rows);
                if (!updated) { segments.Add(segment); continue; }
                using var writer = new SegmentWriter(reader.ArchiveDirectory, index, segment.Start, files, segments);
                await CopyRange(segment.Start, segment.Rows, writer);
            }
            using var appended = new SegmentWriter(reader.ArchiveDirectory, index, table.Rows, files, segments);
            AppendRows(appended);
        }
        return table with { Rows = Count(index), Segments = segments.ToArray() };

        async Task CopyRange(long start, long count, SegmentWriter writer)
        {
            long end = start + count;
            while (start < end)
            {
                var page = await reader.ReadRawPageAsync(index, start, (int)Math.Min(ArchiveTableReader.PageSize, end - start), validateCatalog: false);
                if (page.Count == 0) throw new InvalidDataException("書庫の行読込が進みませんでした。");
                foreach (var raw in page)
                {
                    byte[]? value = changes.TryGetValue((index, start++), out var change) ? change : raw;
                    if (value is not null) writer.Add(value);
                }
            }
        }
        void AppendRows(SegmentWriter writer)
        {
            for (long row = table.Rows; row < Next(index); row++)
                if (changes.GetValueOrDefault((index, row)) is { } value) writer.Add(value);
        }
    }

    private sealed class SegmentWriter(string directory, int table, long start, List<string> files, List<ArchiveSegment> segments) : IDisposable
    {
        private NewDbmsDatabase? database;
        private NewDbmsTransaction? transaction;
        private string? path;
        private long position = start, segmentStart, segmentBytes, transactionBytes;

        public void Add(byte[] row)
        {
            if (database is not null && segmentBytes + row.Length > 96L * 1024 * 1024) Close();
            if (database is null)
            {
                path = Path.Combine(directory, $"table-owner-{Guid.NewGuid():N}.ndb"); files.Add(path);
                database = NewDbmsDatabase.Create(path, Guid.NewGuid(), new NewDbmsResourceLimits(MaxRetainedBytes: 512UL * 1024 * 1024, MaxBufferBytes: 64UL * 1024 * 1024));
                segmentStart = position; segmentBytes = 0;
            }
            if (transaction is not null && transactionBytes + row.Length > 8L * 1024 * 1024) Flush();
            transaction ??= database.BeginTransaction();
            transaction.Put(ArchiveTableReader.RowKey(table, position++), row);
            segmentBytes += row.Length; transactionBytes += row.Length + 64;
        }
        private void Flush()
        {
            using var pending = transaction; transaction = null;
            pending?.Commit(); transactionBytes = 0;
        }
        private void Close()
        {
            if (database is null) return;
            try
            {
                Flush(); database.Checkpoint();
                if (new FileInfo(path!).Length > 256L * 1024 * 1024) throw new IOException("生成した書庫セグメントが256 MiBを超えます。");
                segments.Add(new(Path.GetFileName(path!), segmentStart, position - segmentStart));
            }
            finally { transaction?.Dispose(); transaction = null; database.Dispose(); database = null; }
        }
        public void Dispose() => Close();
    }
}
