using System.Buffers.Binary;
using System.Globalization;
using System.Text;
using NewDbms;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed partial class ArchiveTableReader
{
    public Task<IReadOnlyList<byte[]>> ReadRawPageAsync(int tableIndex, long offset, int count = PageSize, bool validateCatalog = true) => Task.Run<IReadOnlyList<byte[]>>(() =>
    {
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            var table = Manifest.Tables[tableIndex];
            if (offset < 0 || offset > table.Rows || count is < 1 or > PageSize) throw new ArgumentOutOfRangeException(nameof(offset));
            if (validateCatalog) SnapshotFile("library.ndb", 8 * 1024 * 1024);
            var rows = new List<byte[]>(); long position = offset, end = offset + Math.Min(count, table.Rows - offset);
            long bytes = 0;
            foreach (var segment in table.Segments)
            {
                if (segment.Start + segment.Rows <= position || segment.Start >= end) continue;
                if (activeSegment != segment.File) { activeDatabase?.Dispose(); activeDatabase = null; activeSegment = null; }
                var path = SnapshotFile(segment.File);
                if (activeDatabase is null)
                {
                    activeDatabase = NewDbmsDatabase.Open(path, new NewDbmsResourceLimits(MaxRetainedBytes: 512UL * 1024 * 1024));
                    activeSegment = segment.File;
                }
                using var tx = activeDatabase.BeginTransaction();
                while (position < Math.Min(end, segment.Start + segment.Rows))
                {
                    var row = tx.Get(RowKey(tableIndex, position)) ?? throw new InvalidDataException(L.Get("MissingRow", table.Name, position));
                    if (bytes + row.Length > 8 * 1024 * 1024 && rows.Count > 0) return rows;
                    rows.Add(row); bytes += row.Length; position++;
                }
            }
            if (position != end && rows.Count == 0) throw new InvalidDataException("書庫の行読込が進みませんでした。");
            return rows;
        }
    });

    public SqlValue[] DecodeValues(byte[] encoded, int columns)
    {
        bool truncated = false;
        return Decode(encoded, columns).Select(cell => Cell(cell, preview: false, ref truncated)).ToArray();
    }

    public byte[] EncodeValues(SqlValue[] values, byte[]? original = null)
    {
        var cells = original is null ? null : Decode(original, values.Length);
        var previous = original is null ? null : DecodeValues(original, values.Length);
        using var stream = new MemoryStream();
        using var writer = new BinaryWriter(stream, Encoding.UTF8);
        writer.Write(values.Length);
        for (int column = 0; column < values.Length; column++)
        {
            ArchiveCell cell;
            var value = values[column];
            if (previous is not null && previous[column] == value) cell = cells![column];
            else if (value.IsNull) cell = new(0, []);
            else if (value.Kind == "TEXT") cell = new(3, textEncoding.GetBytes(value.Text!));
            else if (value.Kind == "BLOB") cell = new(4, Convert.FromHexString(value.Text!));
            else if (value.Kind == "NUMBER")
            {
                var bytes = new byte[8];
                if (long.TryParse(value.Text, NumberStyles.Integer, CultureInfo.InvariantCulture, out long integer))
                { BinaryPrimitives.WriteInt64LittleEndian(bytes, integer); cell = new(1, bytes); }
                else
                {
                    double real = double.Parse(value.Text!, CultureInfo.InvariantCulture);
                    if (!double.IsFinite(real)) throw new SqlException("書庫へ有限でない数値は保存できません。");
                    BinaryPrimitives.WriteInt64LittleEndian(bytes, BitConverter.DoubleToInt64Bits(real)); cell = new(2, bytes);
                }
            }
            else throw new SqlException($"書庫で保存できない型です: {value.Kind}");
            writer.Write(cell.StorageClass); writer.Write(cell.Data.Length); writer.Write(cell.Data);
        }
        return stream.ToArray();
    }
}
