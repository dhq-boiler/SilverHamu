using System.Security.Cryptography;
using System.Globalization;
using System.Numerics;
using System.Text;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private static string Digest(string text) =>
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(text)));

    private static string SecondaryPrefix(SqlTable table, SqlIndex index) =>
        "_catdb_owner/sql/v1/index/" + TableId(table.Name) + "/" + Digest(index.Name) + "/";

    private sealed class IndexLayout(SqlTable table, SqlIndex index)
    {
        public string Prefix { get; } = SecondaryPrefix(table, index);
        public IReadOnlyList<string> Columns { get; } = index.ColumnNames;
        public bool Ordered { get; } = index.Columns is not null;

        public string Component(SqlValue value)
        {
            if (!Ordered) return Digest(UniqueIdentity(value)) + "/";
            if (value.IsNull) return "0/";
            // Keep the concatenated tuple, separators and 32-byte row ID inside
            // catdb's 1024-byte key budget. Ties from truncation are rechecked.
            int width = Math.Min(128, (1024 - Prefix.Length - 32) / Columns.Count - 2);
            var text = value.Text!;
            string encoded;
            switch (value.Kind)
            {
                case "NUMBER":
                    var bits = decimal.GetBits(decimal.Parse(text, CultureInfo.InvariantCulture));
                    var coefficient = ((BigInteger)(uint)bits[2] << 64) + ((BigInteger)(uint)bits[1] << 32) + (uint)bits[0];
                    var scale = (bits[3] >> 16) & 255;
                    var scaled = coefficient * BigInteger.Pow(10, 28 - scale);
                    var biased = (BigInteger.One << 191) + (bits[3] < 0 ? -scaled : scaled);
                    encoded = Convert.ToHexString(biased.ToByteArray(isUnsigned: true, isBigEndian: true)).PadLeft(48, '0');
                    break;
                case "TEXT":
                    // SQL compares UTF-16 code units ordinally, including surrogates.
                    var chars = new StringBuilder(width + 3);
                    foreach (var ch in text.AsSpan(0, Math.Min(text.Length, (width + 3) / 4)))
                        chars.Append(((ushort)ch).ToString("X4", CultureInfo.InvariantCulture));
                    encoded = chars.ToString();
                    break;
                case "BOOLEAN": encoded = value.IsTrue ? "1" : "0"; break;
                case "BLOB": encoded = text; break;
                default: throw new SqlException("索引列の型が不正です。");
            }
            return "1" + encoded[..Math.Min(encoded.Length, width)] + "/";
        }

        public string? Entry(string rowKey, IReadOnlyDictionary<string, SqlValue>? row)
        {
            if (row is null || !Ordered && row[Columns[0]].IsNull) return null;
            var key = new StringBuilder(Prefix);
            foreach (var name in Columns) key.Append(Component(row[name]));
            return key.Append(rowKey.AsSpan(rowKey.LastIndexOf('/') + 1)).ToString();
        }

        public void RecordChange(string rowKey, IReadOnlyDictionary<string, SqlValue>? before,
            IReadOnlyDictionary<string, SqlValue>? after, Dictionary<string, byte[]?> changes)
        {
            var oldKey = Entry(rowKey, before);
            var newKey = Entry(rowKey, after);
            if (oldKey == newKey) return;
            if (oldKey is not null) changes[oldKey] = null;
            if (newKey is not null) changes[newKey] = [];
        }
    }

    private sealed record IndexCondition(string Column, string Operator, SqlValue Value);
    private sealed record IndexRange(IndexLayout Layout, string Start, string End, int MatchedColumns);

    private static bool CollectIndexConditions(SqlExpression? expression, SqlTable table, List<IndexCondition> conditions)
    {
        if (expression is SqlBinary { Operator: "AND" } and)
            return CollectIndexConditions(and.Left, table, conditions) && CollectIndexConditions(and.Right, table, conditions);
        if (expression is not SqlBinary cmp || cmp.Operator is not ("=" or "<" or "<=" or ">" or ">=")) return false;
        var op = cmp.Operator;
        var field = cmp.Left as SqlField;
        var literal = cmp.Right as SqlLiteral;
        if (field is null || literal is null)
        {
            field = cmp.Right as SqlField;
            literal = cmp.Left as SqlLiteral;
            op = op switch { ">" => "<", "<" => ">", ">=" => "<=", "<=" => ">=", _ => op };
        }
        if (field is null || literal is null) return false;
        var column = Column(table, field.Name);
        var kind = column.Type is "INTEGER" or "REAL" ? "NUMBER" : column.Type;
        if (kind != literal.Value.Kind) return false;
        conditions.Add(new(column.Name, op, literal.Value));
        return true;
    }

    private static IndexRange? FindIndexRange(SqlExpression? where, SqlTable table)
    {
        if (table.Indexes is not { Count: > 0 }) return null;
        var conditions = new List<IndexCondition>();
        if (!CollectIndexConditions(where, table, conditions)) return null;
        IndexRange? best = null;
        foreach (var index in table.Indexes)
        {
            var layout = new IndexLayout(table, index);
            var prefix = layout.Prefix;
            var start = prefix;
            var end = prefix + "~";
            int matched = 0;
            foreach (var name in layout.Columns)
            {
                var terms = conditions.Where(c => c.Column == name).ToList();
                if (terms.Count == 0) break;
                var equal = terms.Find(c => c.Operator == "=");
                if (equal is not null)
                {
                    prefix += layout.Component(equal.Value);
                    start = prefix;
                    end = prefix + "~";
                    matched++;
                    continue;
                }
                if (!layout.Ordered) break;
                foreach (var term in terms)
                {
                    var bound = prefix + layout.Component(term.Value);
                    if (term.Operator is ">" or ">=")
                    {
                        if (string.CompareOrdinal(bound, start) > 0) start = bound;
                    }
                    else if (string.CompareOrdinal(bound + "~", end) < 0) end = bound + "~";
                }
                matched++;
                break;
            }
            if (matched > (best?.MatchedColumns ?? 0)) best = new(layout, start, end, matched);
        }
        return best;
    }

    private async Task<SqlResult> CreateIndexAsync(SqlTable table, CreateIndex create, List<(string Key, byte[]? Value)> changes)
    {
        var indexes = table.Indexes ?? [];
        if (indexes.Exists(i => i.Name.Equals(create.Name, StringComparison.OrdinalIgnoreCase)))
        {
            if (create.IfNotExists) return Empty("索引は既に存在します。変更なし。");
            throw new SqlException($"索引 '{create.Name}' は既に存在します。");
        }
        var columns = create.Columns.Select(c => Column(table, c).Name).ToList();
        var index = new SqlIndex(create.Name, columns[0], columns);
        var layout = new IndexLayout(table, index);
        foreach (var row in await LoadRowsAsync(table))
            changes.Add((layout.Entry(row.StorageKey, row.Values)!, []));
        changes.Add((TableKey(table.Name), Serialize(table with { Indexes = [.. indexes, index] })));
        return Empty($"索引 '{index.Name}' を作成しました。");
    }

    private async Task<SqlResult> DropIndexAsync(SqlTable table, DropIndex drop, List<(string Key, byte[]? Value)> changes)
    {
        if (table.CompositePrimaryKey is not null && drop.Name.Equals("PRIMARY", StringComparison.OrdinalIgnoreCase))
            throw new SqlException("複合主キーの索引を直接削除することはできません。");
        var index = table.Indexes?.Find(i => i.Name.Equals(drop.Name, StringComparison.OrdinalIgnoreCase));
        if (index is null)
        {
            if (drop.IfExists) return Empty("索引は存在しません。変更なし。");
            throw new SqlException($"索引 '{drop.Name}' は存在しません。");
        }
        foreach (var entry in await ScanAsync(SecondaryPrefix(table, index), MaxRows))
            changes.Add((entry.Key, null));
        changes.Add((TableKey(table.Name), Serialize(table with { Indexes = table.Indexes!.Where(i => i != index).ToList() })));
        return Empty($"索引 '{index.Name}' を削除しました。");
    }

    private async Task<SqlResult> SelectSecondaryAsync(SelectRows query, SqlTable table, IndexRange range)
    {
        if (string.CompareOrdinal(range.Start, range.End) >= 0) return Select(query, table, []);
        var rowPrefix = RowPrefix(table.Name);
        var rows = new List<SqlRow>();
        var bytesRead = 0;
        foreach (var entry in await ScanRangeAsync(range.Start, range.End, MaxRows))
        {
            var rowId = entry.Key[(entry.Key.LastIndexOf('/') + 1)..];
            if (!Guid.TryParseExact(rowId, "N", out _)) throw new SqlException("索引の行IDが不正です。");
            var key = rowPrefix + rowId;
            var bytes = await GetAsync(key) ?? throw new SqlException("索引の参照先がありません。");
            bytesRead += bytes.Length;
            if (bytesRead > MaxReadBytes) throw new SqlException("索引検索の読込上限（8 MiB）を超えています。");
            var row = DecodeRow(key, bytes);
            if (range.Layout.Entry(key, row.Values) != entry.Key) throw new SqlException("索引と行の値が一致しません。");
            rows.Add(row);
        }
        return Select(query, table, rows);
    }
}
