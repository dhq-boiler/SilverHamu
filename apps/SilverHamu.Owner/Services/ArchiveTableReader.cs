using System.Buffers.Binary;
using System.Globalization;
using System.Text;
using System.Text.Json;
using SilverHamu.Sql;
using NewDbms;

namespace SilverHamu.Owner.Services;

internal sealed record ArchiveColumn(string Name, string? DeclaredType);
internal sealed record ArchiveSegment(string File, long Start, long Rows);
internal sealed record ArchiveTable(string Name, ArchiveColumn[] Columns, long Rows, ArchiveSegment[] Segments);
internal sealed record ArchiveCatalog(int Format, string TextEncoding, ArchiveTable[] Tables);
internal sealed record ArchiveCell(byte StorageClass, byte[] Data);

internal sealed record ArchiveTablePage(int TableIndex, long Offset, long NextOffset, long TotalRows, SqlResult Result, bool PreviewTruncated);

internal sealed class ArchiveTableReader : IDisposable
{
    public const int PageSize = 100;
    private const long CacheLimit = 512L * 1024 * 1024;
    private const long FileLimit = 256L * 1024 * 1024;
    private readonly object gate = new();
    private readonly string cacheDirectory;
    private readonly Encoding textEncoding;
    private readonly Dictionary<string, Snapshot> snapshots = new(StringComparer.OrdinalIgnoreCase);
    private long cachedBytes;
    private long access;
    private bool disposed;
    private NewDbmsDatabase? activeDatabase;
    private string? activeSegment;


    public string ArchiveDirectory { get; }
    public ArchiveCatalog Manifest { get; }

    private sealed record Snapshot(string Path, long Bytes, DateTime SourceWriteTime, long Used);

    private sealed class MissingCatalogException : Exception { }

    public static ArchiveTableReader? TryOpen(string directory)
    {
        try { return new ArchiveTableReader(directory); }
        catch (MissingCatalogException) { return null; }
    }

    private ArchiveTableReader(string directory)
    {
        
        ArchiveDirectory = directory;
        cacheDirectory = Path.Combine(Path.GetTempPath(), "silverhamu-owner-palv-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(cacheDirectory);
        try
        {
            SnapshotFile("library.ndb", 8 * 1024 * 1024);
            using (var database = NewDbmsDatabase.Open(Path.Combine(cacheDirectory, "library.ndb"), new NewDbmsResourceLimits()))
            using (var transaction = database.BeginTransaction())
                Manifest = JsonSerializer.Deserialize<ArchiveCatalog>(transaction.Get("palv/import/manifest/v1"u8.ToArray()) ?? throw new MissingCatalogException()) ?? throw new InvalidDataException("PALVマニフェストが不正です。");
            ValidateManifest(Manifest);
            textEncoding = Manifest.TextEncoding switch
            {
                "UTF-8" => new UTF8Encoding(false, true),
                "UTF-16le" => new UnicodeEncoding(false, false, true),
                "UTF-16be" => new UnicodeEncoding(true, false, true),
                _ => throw new InvalidDataException("PALVアーカイブの文字エンコーディングが未対応です。")
            };
        }
        catch { Directory.Delete(cacheDirectory, recursive: true); throw; }
    }

    private static void RejectLinks(string root, string path)
    {
        FileSystemInfo? item = Directory.Exists(path) ? new DirectoryInfo(path) : new FileInfo(path);
        while (item is not null && item.FullName.Length >= root.Length)
        {
            if (item.Exists && (item.Attributes & FileAttributes.ReparsePoint) != 0) throw new InvalidDataException("リンク・リパースポイントを経由するPALV参照は使用できません。");
            item = item is DirectoryInfo directory ? directory.Parent : ((FileInfo)item).Directory;
        }
    }

    private static void ValidateManifest(ArchiveCatalog manifest)
    {
        if (manifest.Format != 1 || manifest.Tables is null || manifest.Tables.Length > 4096 || manifest.Tables.Select(x => x.Name).Distinct(StringComparer.OrdinalIgnoreCase).Count() != manifest.Tables.Length)
            throw new InvalidDataException("PALVのテーブル定義が不正です。");
        foreach (var table in manifest.Tables)
        {
            if (string.IsNullOrWhiteSpace(table.Name) || table.Columns is null || table.Columns.Length is < 1 or > 512 || table.Rows < 0 || table.Segments is null || table.Columns.Any(c => string.IsNullOrWhiteSpace(c.Name))) throw new InvalidDataException("PALVの列定義または行数が不正です。");
            long expected = 0;
            foreach (var segment in table.Segments)
            {
                if (string.IsNullOrWhiteSpace(segment.File) || Path.GetFileName(segment.File) != segment.File || segment.File.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 || segment.File.Contains('/') || segment.File.Contains('\\') || !segment.File.StartsWith("table-", StringComparison.Ordinal) || !segment.File.EndsWith(".ndb", StringComparison.Ordinal) || segment.Start != expected || segment.Rows < 0 || segment.Rows > table.Rows - expected)
                    throw new InvalidDataException("PALVの分割ファイル定義が不正です。");
                expected += segment.Rows;
            }
            if (expected != table.Rows) throw new InvalidDataException("PALVのテーブル行数と分割ファイル範囲が一致しません。");
        }
    }

    private string SnapshotFile(string file, long maxBytes = FileLimit)
    {
        var source = Path.Combine(ArchiveDirectory, file);
        RejectLinks(ArchiveDirectory, source);
        var info = new FileInfo(source);
        if (!info.Exists) throw new FileNotFoundException("PALVの分割ファイルが見つかりません。", file);
        if (info.Length > maxBytes) throw new InvalidDataException($"閲覧用コピーのファイル上限を超えています: {file}");
        if (snapshots.TryGetValue(file, out var cached))
        {
            if (info.Length != cached.Bytes || info.LastWriteTimeUtc != cached.SourceWriteTime) throw new InvalidDataException("閲覧中にPALVアーカイブが変更されました。参照ファイルを開き直してください。");
            snapshots[file] = cached with { Used = ++access }; return cached.Path;
        }
        while (cachedBytes + info.Length > CacheLimit)
        {
            var oldest = snapshots.Where(x => x.Key != "library.ndb").OrderBy(x => x.Value.Used).First();
            File.Delete(oldest.Value.Path);
            var owner = oldest.Value.Path + ".owner"; if (File.Exists(owner)) File.Delete(owner);
            cachedBytes -= oldest.Value.Bytes; snapshots.Remove(oldest.Key);
        }
        var target = Path.Combine(cacheDirectory, file);
        var expectedLength = info.Length; var expectedWriteTime = info.LastWriteTimeUtc;
        try
        {
            // Opening originals with the native API can recover/truncate a WAL; only private copies may be opened that way.
            using var input = new FileStream(source, FileMode.Open, FileAccess.Read, FileShare.Read);
            using var output = new FileStream(target, FileMode.CreateNew, FileAccess.Write, FileShare.None);
            if (input.Length != info.Length) throw new IOException("コピー開始時にアーカイブが変更されました。");
            input.CopyTo(output);
            info.Refresh();
            if (info.Length != expectedLength || info.LastWriteTimeUtc != expectedWriteTime) throw new IOException("コピー中にアーカイブが変更されました。");
        }
        catch (IOException error)
        {
            if (File.Exists(target)) File.Delete(target);
            throw new IOException($"アーカイブを安定した状態で読み取れません: {file}。他のプロセスで使用中かファイルが変更されています。", error);
        }
        snapshots.Add(file, new(target, expectedLength, expectedWriteTime, ++access)); cachedBytes += expectedLength;
        return target;
    }

    public Task<ArchiveTablePage> ReadPageAsync(int tableIndex, long offset, int count = PageSize, bool preview = true) => Task.Run(() =>
    {
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            if ((uint)tableIndex >= (uint)Manifest.Tables.Length || count is < 1 or > 100 || offset < 0) throw new ArgumentOutOfRangeException(nameof(tableIndex));
            var table = Manifest.Tables[tableIndex];
            if (offset > table.Rows) throw new ArgumentOutOfRangeException(nameof(offset));
            SnapshotFile("library.ndb", 8 * 1024 * 1024);
            var values = new List<SqlValue[]>(); var position = offset; var bytes = 0L; var truncated = false;
            var requestedEnd = offset + Math.Min(count, table.Rows - offset);
            foreach (var segment in table.Segments)
            {
                var segmentEnd = segment.Start + segment.Rows;
                if (segmentEnd <= position || segment.Start >= requestedEnd) continue;
                if (activeSegment != segment.File)
                {
                    activeDatabase?.Dispose(); activeDatabase = null; activeSegment = null;
                }
                var path = SnapshotFile(segment.File);
                if (activeDatabase is null)
                {
                    activeDatabase = NewDbmsDatabase.Open(path, new NewDbmsResourceLimits(MaxRetainedBytes: 512UL * 1024 * 1024));
                    activeSegment = segment.File;

                }
                var database = activeDatabase;
                using var transaction = database.BeginTransaction();
                while (position < Math.Min(segmentEnd, requestedEnd))
                {
                    var encoded = transaction.Get(RowKey(tableIndex, position)) ?? throw new InvalidDataException($"行が欠落しています: {table.Name} / {position}");
                    if (bytes + encoded.Length > 8 * 1024 * 1024 && values.Count > 0) return Page();
                    bytes += encoded.Length;
                    var row = Decode(encoded, table.Columns.Length);
                    values.Add(row.Select(cell => Cell(cell, preview, ref truncated)).ToArray()); position++;
                }
            }
            return Page();

            ArchiveTablePage Page() => new(tableIndex, offset, position, table.Rows, new(table.Columns.Select(c => c.Name).ToArray(), values, $"{table.Name}: {values.Count} 行（全{table.Rows:N0}行）"), truncated);
        }
    });

    private static byte[] RowKey(int table, long row)
    {
        var key = new byte[13]; key[0] = 1;
        System.Buffers.Binary.BinaryPrimitives.WriteInt32BigEndian(key.AsSpan(1), table);
        System.Buffers.Binary.BinaryPrimitives.WriteInt64BigEndian(key.AsSpan(5), row);
        return key;
    }

    private static ArchiveCell[] Decode(byte[] bytes, int columns)
    {
        using var reader = new BinaryReader(new MemoryStream(bytes), Encoding.UTF8);
        if (reader.ReadInt32() != columns) throw new InvalidDataException("Column count mismatch.");
        var cells = new ArchiveCell[columns];
        for (var i = 0; i < columns; i++)
        {
            var kind = reader.ReadByte(); var length = reader.ReadInt32();
            if (kind > 4 || length < 0 || length > reader.BaseStream.Length - reader.BaseStream.Position ||
                (kind == 0 && length != 0) || ((kind == 1 || kind == 2) && length != 8))
                throw new InvalidDataException("Invalid cell encoding.");
            cells[i] = new ArchiveCell(kind, reader.ReadBytes(length));
        }
        if (reader.BaseStream.Position != reader.BaseStream.Length) throw new InvalidDataException("Trailing row bytes.");
        return cells;
    }

    private SqlValue Cell(ArchiveCell cell, bool preview, ref bool truncated)
    {
        switch (cell.StorageClass)
        {
            case 0: return SqlValue.Null;
            case 1: return new("NUMBER", BinaryPrimitives.ReadInt64LittleEndian(cell.Data).ToString(CultureInfo.InvariantCulture));
            case 2: return new("NUMBER", BitConverter.Int64BitsToDouble(BinaryPrimitives.ReadInt64LittleEndian(cell.Data)).ToString("R", CultureInfo.InvariantCulture));
            case 3:
                var text = textEncoding.GetString(cell.Data);
                if (preview && text.Length > 4096) { truncated = true; var end = char.IsHighSurrogate(text[4095]) ? 4095 : 4096; return SqlValue.String(text[..end] + $"… ({text.Length:N0}文字)"); }
                return SqlValue.String(text);
            case 4:
                if (preview && cell.Data.Length > 256) { truncated = true; return new("BLOB", Convert.ToHexString(cell.Data.AsSpan(0, 256)) + $"… ({cell.Data.Length:N0} bytes)"); }
                return new("BLOB", Convert.ToHexString(cell.Data));
            default: throw new InvalidDataException("PALVの保存型が不正です。");
        }
    }

    public void Dispose()
    {
        lock (gate)
        {
            if (disposed) return;
            disposed = true;
            activeDatabase?.Dispose(); activeDatabase = null; activeSegment = null;
            Directory.Delete(cacheDirectory, recursive: true);
        }
    }
}
