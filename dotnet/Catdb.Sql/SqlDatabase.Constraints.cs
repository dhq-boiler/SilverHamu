namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private static bool SameTable(string one, string other) => SqlNames.Canonical(one) == SqlNames.Canonical(other);

    private async Task ValidateChecksAsync(SqlTable table, IReadOnlyList<SqlRow> rows)
    {
        var definitions = (table.Checks ?? []).Concat(table.Columns.SelectMany(c => c.Checks ?? []));
        foreach (var text in definitions)
        {
            var expression = SqlParser.ParseCheck(text, out var bindings);
            await BindExpressionsAsync(bindings);
            ValidateCheckExpression(expression, table.Columns);
            foreach (var row in rows)
            {
                var value = expression.Evaluate(row.Values);
                // SQL CHECK accepts TRUE or UNKNOWN.
                if (!value.IsNull && !value.IsTrue) throw new SqlException($"CHECK ({text}) に違反しています。");
            }
        }
    }

    private static void ValidateCheckExpression(SqlExpression expression, IReadOnlyList<SqlColumn> columns)
    {
        expression.Validate(columns);
        if (expression.ContainsAggregate() || ExpressionType(expression, columns) is not ("BOOLEAN" or "NULL"))
            throw new SqlException("CHECKには条件式を指定してください。");
    }

    private static IEnumerable<SqlForeignKey> ForeignKeys(SqlTable table)
    {
        foreach (var key in table.ForeignKeys ?? []) yield return key;
        foreach (var column in table.Columns)
            if (column.Reference is { } reference)
                yield return new SqlForeignKey([column.Name], reference.Table, [reference.Column], reference.DeleteCascade, reference.UpdateCascade)
                    { OnDelete = reference.OnDelete, OnUpdate = reference.OnUpdate };
    }

    // Length prefixes distinguish tuples even when text contains delimiters. MATCH SIMPLE
    // exempts a referencing row if any component is NULL.
    private static string? ReferenceIdentity(SqlRow row, IReadOnlyList<string> columns)
    {
        var key = new System.Text.StringBuilder();
        foreach (var name in columns)
        {
            var value = row.Values[name];
            if (value.IsNull) return null;
            var part = UniqueIdentity(value);
            key.Append(part.Length).Append(':').Append(part);
        }
        return key.ToString();
    }

    private static void ValidateForeignKey(SqlTable child, SqlForeignKey fk, SqlTable parent)
    {
        var uniqueTarget = fk.Targets.Count == 1 ? Column(parent, fk.Targets[0]).Unique :
            parent.CompositePrimaryKey is { } pk && pk.Count == fk.Targets.Count &&
            pk.All(name => fk.Targets.Contains(name, StringComparer.OrdinalIgnoreCase));
        uniqueTarget |= UniqueIndexes(parent).Any(index => index.Predicate is null && index.ColumnNames.Count == fk.Targets.Count &&
            index.ColumnNames.All(name => fk.Targets.Contains(name, StringComparer.OrdinalIgnoreCase)));
        if (!uniqueTarget || fk.Columns.Count != fk.Targets.Count)
            throw new SqlException($"外部キー '{child.Name}' の参照先には主キーまたはUNIQUE列を指定してください。");
        for (int i = 0; i < fk.Columns.Count; i++)
            if (ComparisonType(Column(child, fk.Columns[i])) != ComparisonType(Column(parent, fk.Targets[i])))
                throw new SqlException($"外部キー '{child.Name}.{fk.Columns[i]}' の型が参照先と一致しません。");
    }

    private async Task ValidateOutgoingReferencesAsync(SqlTable child, IReadOnlyCollection<SqlRow> rows,
        IReadOnlyDictionary<string, RowSet>? states = null)
    {
        foreach (var fk in ForeignKeys(child))
        {
            bool self = SameTable(child.Name, fk.Table);
            RowSet? state = null;
            states?.TryGetValue(fk.Table, out state);
            var parent = self ? child : state?.Table ?? await ReadSourceTableAsync(fk.Table);
            ValidateForeignKey(child, fk, parent);
            if (rows.Count == 0) continue;
            IEnumerable<SqlRow> parents = self ? rows : state is not null ? state.Current.Values : await LoadRowsAsync(parent);
            var valid = parents.Select(r => ReferenceIdentity(r, fk.Targets)).ToHashSet(StringComparer.Ordinal);
            foreach (var row in rows)
                if (ReferenceIdentity(row, fk.Columns) is { } key && !valid.Contains(key))
                    throw new SqlException($"外部キー '{child.Name}({string.Join(", ", fk.Columns)})' の参照先がありません。");
        }
    }

    private async Task RestrictReferencedSchemaAsync(SqlTable table, string? targetColumn)
    {
        foreach (var child in await GetTablesAsync())
        {
            if (targetColumn is null && SameTable(table.Name, child.Name)) continue;
            foreach (var fk in ForeignKeys(child))
                if (SameTable(table.Name, fk.Table) &&
                    (targetColumn is null || fk.Targets.Contains(targetColumn, StringComparer.OrdinalIgnoreCase)))
                    throw new SqlException($"外部キー '{child.Name}({string.Join(", ", fk.Columns)})' が参照しています。");
        }
    }
}
