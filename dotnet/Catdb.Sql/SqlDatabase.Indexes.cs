using System.Security.Cryptography;
using System.Runtime.CompilerServices;
using System.Globalization;
using System.Numerics;
using System.Text;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private static readonly ConditionalWeakTable<SqlIndex, SqlExpression> PartialIndexPredicates = new();

    private static SqlExpression IndexPredicate(SqlIndex index) =>
        PartialIndexPredicates.GetValue(index, static i => SqlParser.ParseCheck(i.Predicate!));

    private static bool IndexIncludes(SqlIndex index, IReadOnlyDictionary<string, SqlValue> row) =>
        index.Predicate is null || IndexPredicate(index).Evaluate(row).IsTrue;

    private static void ValidateIndexPredicate(SqlIndex index, SqlTable table)
    {
        if (index.Predicate is null) return;
        var expression = IndexPredicate(index);
        expression.Rewrite(node =>
        {
            if (node is not (SqlField or SqlLiteral or SqlIsNull or SqlBetween or SqlInList or
                SqlBinary { Operator: "=" or "<>" or "!=" or "<" or ">" or "<=" or ">=" or "AND" or "OR" or "LIKE" } or
                SqlUnary { Operator: "NOT" }))
                throw new SqlException("Index predicates support comparisons and boolean conditions on table columns.");
            return null;
        });
        ValidatePredicate(expression, table.Columns);
    }

    private static bool ImpliesPredicate(SqlExpression? condition, SqlExpression predicate)
    {
        if (SameIndexPredicate(condition, predicate)) return true;
        if (predicate is SqlBinary { Operator: "AND" } required)
            return ImpliesPredicate(condition, required.Left) && ImpliesPredicate(condition, required.Right);
        if (condition is SqlBinary { Operator: "OR" } alternatives)
            return ImpliesPredicate(alternatives.Left, predicate) && ImpliesPredicate(alternatives.Right, predicate);
        if (predicate is SqlIsNull { Negated: true, Expression: SqlField field } &&
            condition is SqlBinary { Operator: "=" or "<>" or "!=" or "<" or ">" or "<=" or ">=" or "LIKE" } comparison)
            return SameIndexPredicate(comparison.Left, field) || SameIndexPredicate(comparison.Right, field);
        return condition is SqlBinary { Operator: "AND" } conjunction &&
            (ImpliesPredicate(conjunction.Left, predicate) || ImpliesPredicate(conjunction.Right, predicate));
    }

    private static bool SameIndexPredicate(SqlExpression? first, SqlExpression second) => (first, second) switch
    {
        (SqlField a, SqlField b) => string.Equals(a.Qualifier, b.Qualifier, StringComparison.OrdinalIgnoreCase)
            && string.Equals(a.Name, b.Name, StringComparison.OrdinalIgnoreCase),
        (SqlBinary a, SqlBinary b) => a.Operator == b.Operator &&
            SameIndexPredicate(a.Left, b.Left) && SameIndexPredicate(a.Right, b.Right),
        (SqlUnary a, SqlUnary b) => a.Operator == b.Operator && SameIndexPredicate(a.Operand, b.Operand),
        (SqlIsNull a, SqlIsNull b) => a.Negated == b.Negated && SameIndexPredicate(a.Expression, b.Expression),
        (SqlBetween a, SqlBetween b) => SameIndexPredicate(a.Value, b.Value) && SameIndexPredicate(a.Lower, b.Lower)
            && SameIndexPredicate(a.Upper, b.Upper),
        (SqlInList a, SqlInList b) => SameIndexPredicate(a.Value, b.Value) && a.Items.Count == b.Items.Count &&
            a.Items.Zip(b.Items).All(pair => SameIndexPredicate(pair.First, pair.Second)),
        _ => first == second
    };

    private static IEnumerable<SqlIndex> UniqueIndexes(SqlTable table) => (table.Indexes ?? []).Where(i => i.Unique);

    private static string? UniqueIndexIdentity(SqlIndex index, SqlRow row) =>
        IndexIncludes(index, row.Values) ? ReferenceIdentity(row, index.ColumnNames) : null;

    private static void ValidateUniqueIndex(SqlIndex index, IEnumerable<SqlRow> rows)
    {
        var keys = new HashSet<string>(StringComparer.Ordinal);
        foreach (var row in rows)
            if (UniqueIndexIdentity(index, row) is { } key && !keys.Add(key))
                throw new SqlException($"Duplicate key for unique index '{index.Name}'.");
    }

    private static string Digest(string text) =>
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(text)));

    private static string SecondaryPrefix(SqlTable table, SqlIndex index) =>
        "_catdb_owner/sql/v1/index/" + TableId(table.Name) + "/" + Digest(index.Name) + "/";

    private sealed class IndexLayout(SqlTable table, SqlIndex index)
    {
        public string Name => index.Name;
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
                    var biased = (BigInteger.One << 191) + SqlNumbers.UnitsAtScale28(decimal.Parse(text, CultureInfo.InvariantCulture));
                    encoded = Convert.ToHexString(biased.ToByteArray(isUnsigned: true, isBigEndian: true)).PadLeft(48, '0');
                    break;
                case "TEXT":
                case "UUID":
                case "DATE":
                case "TIME":
                case "LOCAL_DATETIME":
                case "INSTANT":
                    // SQL compares UTF-16 code units ordinally, including surrogates.
                    var chars = new StringBuilder(width + 3);
                    foreach (var ch in text.AsSpan(0, Math.Min(text.Length, (width + 3) / 4)))
                        chars.Append(((ushort)ch).ToString("X4", CultureInfo.InvariantCulture));
                    encoded = chars.ToString();
                    break;
                case "BOOLEAN": encoded = value.IsTrue ? "1" : "0"; break;
                case "BLOB": encoded = text; break;
                case "DURATION":
                    var ticks = SqlTypeRules.DurationTicks(value);
                    encoded = (unchecked((ulong)ticks) ^ (1UL << 63)).ToString("X16", CultureInfo.InvariantCulture);
                    break;
                default: throw new SqlException("索引列の型が不正です。");
            }
            return "1" + encoded[..Math.Min(encoded.Length, width)] + "/";
        }

        public string? Entry(string rowKey, IReadOnlyDictionary<string, SqlValue>? row)
        {
            if (row is null || !Ordered && row[Columns[0]].IsNull || !IndexIncludes(index, row)) return null;
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
    private sealed record IndexRange(IndexLayout Layout, string Start, string End, double EstimatedRows);

    private static SqlValue? IndexLiteral(SqlExpression expression) => expression switch
    {
        SqlLiteral literal => literal.Value,
        SqlCast { Operand: SqlLiteral } cast => cast.Evaluate(new Dictionary<string, SqlValue>()),
        _ => null
    };

    private static bool MatchesIndexType(SqlColumn column, SqlValue value) =>
        ComparisonType(column) == (value.TypeName ?? value.Kind);

    private static IndexCondition? LiteralComparison(SqlExpression? expression, SqlTable table)
    {
        if (expression is not SqlBinary cmp || cmp.Operator is not ("=" or "<" or "<=" or ">" or ">=")) return null;
        var op = cmp.Operator;
        var field = cmp.Left as SqlField;
        SqlValue? value;
        if (field is not null) value = IndexLiteral(cmp.Right);
        else
        {
            field = cmp.Right as SqlField;
            value = field is null ? null : IndexLiteral(cmp.Left);
            op = op switch { ">" => "<", "<" => ">", ">=" => "<=", "<=" => ">=", _ => op };
        }
        if (field is null || value is null) return null;
        var column = Column(table, field.Name);
        return MatchesIndexType(column, value) ? new(column.Name, op, value) : null;
    }

    private static void CollectIndexConditions(SqlExpression? expression, SqlTable table, List<IndexCondition> conditions)
    {
        if (expression is SqlBinary { Operator: "AND" } and)
        {
            CollectIndexConditions(and.Left, table, conditions);
            CollectIndexConditions(and.Right, table, conditions);
        }
        else if (LiteralComparison(expression, table) is { } term)
            conditions.Add(term);
    }

    private static IEnumerable<IndexRange> CandidateIndexRanges(SqlTable table, List<IndexCondition> conditions, long rowCount, SqlExpression? where)
    {
        foreach (var index in table.Indexes ?? [])
        {
            if (index.Predicate is not null && !ImpliesPredicate(where, IndexPredicate(index))) continue;
            var layout = new IndexLayout(table, index);
            var prefix = layout.Prefix;
            var start = prefix;
            var end = prefix + "~";
            int matched = 0;
            double estimated = rowCount;
            foreach (var name in layout.Columns)
            {
                var terms = conditions.Where(c => c.Column == name).ToList();
                if (terms.Count == 0) break;
                var equal = terms.Find(c => c.Operator == "=");
                if (equal is not null)
                {
                    estimated *= EstimateEqualityFraction(table, name, equal.Value);
                    prefix += layout.Component(equal.Value);
                    start = prefix;
                    end = prefix + "~";
                    matched++;
                    continue;
                }
                if (!layout.Ordered) break;
                estimated *= terms.Any(t => t.Operator is "<" or "<=") && terms.Any(t => t.Operator is ">" or ">=") ? 0.125 : 0.333;
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
            if (matched > 0)
                yield return new(layout, start, end, rowCount == 0 ? 0 : Math.Max(1, estimated));
        }
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
        var index = new SqlIndex(create.Name, columns[0], columns) { Unique = create.Unique, Predicate = create.Predicate };
        ValidateIndexPredicate(index, table);
        var rows = await LoadRowsAsync(table);
        if (index.Unique) ValidateUniqueIndex(index, rows);
        var layout = new IndexLayout(table, index);
        foreach (var row in rows)
            if (layout.Entry(row.StorageKey, row.Values) is { } key) changes.Add((key, []));
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
        var updated = table with { Indexes = table.Indexes!.Where(i => i != index).ToList() };
        if (index.Unique && index.Predicate is null)
            foreach (var child in await GetTablesAsync())
                foreach (var fk in ForeignKeys(child).Where(fk => SameTable(fk.Table, table.Name)))
                    ValidateForeignKey(child, fk, updated);
        foreach (var entry in await ScanAsync(SecondaryPrefix(table, index), MaxRows))
            changes.Add((entry.Key, null));
        changes.Add((TableKey(table.Name), Serialize(updated)));
        return Empty($"索引 '{index.Name}' を削除しました。");
    }

    private async Task<List<SqlRow>> LoadSecondaryRowsAsync(SqlTable table, IndexRange range)
    {
        if (string.CompareOrdinal(range.Start, range.End) >= 0) return [];
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
        return rows;
    }
}
