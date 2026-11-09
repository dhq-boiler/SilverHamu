namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private static bool SameTable(string one, string other) => SqlNames.Canonical(one) == SqlNames.Canonical(other);

    private static void ValidateChecks(SqlTable table, IReadOnlyList<SqlRow> rows)
    {
        var definitions = (table.Checks ?? []).Concat(table.Columns.SelectMany(c => c.Checks ?? []));
        foreach (var text in definitions)
        {
            var expression = SqlParser.ParseCheck(text);
            expression.Validate(table.Columns);
            if (CheckKind(table, expression) is not ("BOOLEAN" or "NULL"))
                throw new SqlException("CHECKには条件式を指定してください。");
            foreach (var row in rows)
            {
                var value = expression.Evaluate(row.Values);
                // SQL CHECK accepts TRUE or UNKNOWN.
                if (!value.IsNull && !value.IsTrue) throw new SqlException($"CHECK ({text}) に違反しています。");
            }
        }
    }

    private static string CheckKind(SqlTable table, SqlExpression expression)
    {
        switch (expression)
        {
            case SqlLiteral literal: return literal.Value.Kind;
            case SqlField field: return ComparisonType(Column(table, field.Name));
            case SqlIsNull test:
                CheckKind(table, test.Expression);
                return "BOOLEAN";
            case SqlBinary binary:
                var left = CheckKind(table, binary.Left);
                var right = CheckKind(table, binary.Right);
                var valid = binary.Operator switch
                {
                    "AND" or "OR" => left is "BOOLEAN" or "NULL" && right is "BOOLEAN" or "NULL",
                    "LIKE" => left is "TEXT" or "NULL" && right is "TEXT" or "NULL",
                    _ => left == right || left == "NULL" || right == "NULL"
                };
                if (!valid) throw new SqlException("CHECK式の列・値の型が一致しません。");
                return "BOOLEAN";
            default: throw new SqlException("未対応のCHECK式です。");
        }
    }

    // Check the complete proposed statement state. Self-references can therefore target
    // another row in the same INSERT, regardless of the order of its VALUES clauses.
    private async Task ValidateOutgoingReferencesAsync(SqlTable child, IReadOnlyList<SqlRow> rows)
    {
        foreach (var column in child.Columns)
        {
            if (column.Reference is not { } reference) continue;
            var self = SameTable(child.Name, reference.Table);
            var parent = self ? child : await ReadSourceTableAsync(reference.Table);
            var target = Column(parent, reference.Column);
            if (!target.Unique || ComparisonType(column) != ComparisonType(target))
                throw new SqlException($"外部キー '{column.Name}' には同じ型の主キーまたはUNIQUE列が必要です。");
            if (rows.Count == 0) continue;
            var parents = self ? rows : await LoadRowsAsync(parent);
            CheckReferences(child, column, rows, target, parents);
        }
    }

    private static void CheckReferences(SqlTable child, SqlColumn column, IReadOnlyList<SqlRow> children,
        SqlColumn target, IReadOnlyList<SqlRow> parents)
    {
        var valid = new HashSet<string>(StringComparer.Ordinal);
        foreach (var row in parents)
        {
            var value = row.Values[target.Name];
            if (!value.IsNull) valid.Add(UniqueIdentity(value));
        }
        foreach (var row in children)
        {
            var value = row.Values[column.Name];
            if (!value.IsNull && !valid.Contains(UniqueIdentity(value)))
                throw new SqlException($"外部キー '{child.Name}.{column.Name}' の参照先がありません。");
        }
    }

    private async Task ValidateIncomingReferencesAsync(SqlTable parent, IReadOnlyList<SqlRow> parents, UpdateRows? update)
    {
        if (!parent.Columns.Any(c => c.Unique && (update is null || update.Values.ContainsKey(c.Name)))) return;
        foreach (var child in await GetTablesAsync())
        {
            IReadOnlyList<SqlRow>? children = null;
            foreach (var column in child.Columns)
            {
                if (column.Reference is not { } reference || !SameTable(reference.Table, parent.Name)) continue;
                var target = Column(parent, reference.Column);
                if (update is not null && !update.Values.ContainsKey(target.Name)) continue;
                children ??= SameTable(child.Name, parent.Name) ? parents : await LoadRowsAsync(child);
                CheckReferences(child, column, children, target, parents);
            }
        }
    }

    private async Task RestrictReferencedSchemaAsync(SqlTable table, string? targetColumn)
    {
        if (!table.Columns.Any(c => c.Unique && (targetColumn is null || c.Name.Equals(targetColumn, StringComparison.OrdinalIgnoreCase)))) return;
        foreach (var child in await GetTablesAsync())
        {
            // A table's own foreign keys disappear when it is dropped.
            if (targetColumn is null && SameTable(table.Name, child.Name)) continue;
            foreach (var column in child.Columns)
                if (column.Reference is { } reference && SameTable(table.Name, reference.Table) &&
                    (targetColumn is null || reference.Column.Equals(targetColumn, StringComparison.OrdinalIgnoreCase)))
                    throw new SqlException($"外部キー '{child.Name}.{column.Name}' が参照しています。");
        }
    }
}
