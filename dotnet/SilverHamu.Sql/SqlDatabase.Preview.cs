namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    public static bool CanPreviewWrite(SqlStatement statement, string table) => statement switch
    {
        UpdateRows update => update.Where is not null && string.Equals(update.Table, table, StringComparison.OrdinalIgnoreCase),
        DeleteRows delete => delete.Where is not null && string.Equals(delete.Table, table, StringComparison.OrdinalIgnoreCase),
        InsertStatement insert => insert.RowCount == 1 && insert.Conflict is null && string.Equals(insert.Table, table, StringComparison.OrdinalIgnoreCase),
        _ => false
    };

    public static SqlValue[] PreviewInsert(SqlTable table, InsertStatement insert)
    {
        if (!string.Equals(insert.Table, table.Name, StringComparison.OrdinalIgnoreCase) || insert.RowCount != 1 || insert.Conflict is not null)
            throw new SqlException("Preview requires a single-row INSERT of the selected table.");
        BindPreviewTypes(table, insert);
        var columns = InsertColumns(table, insert.Columns);
        var values = InsertValuesForColumns(insert, columns).Single();
        var row = MakeInsertedRow(table, columns, values);
        return table.Columns.Select(column => row.Values[column.Name]).ToArray();
    }

    public static SqlValue[] PreviewDelete(SqlTable table, IReadOnlyList<SqlValue[]> rows, DeleteRows delete)
    {
        if (!string.Equals(delete.Table, table.Name, StringComparison.OrdinalIgnoreCase) || delete.Where is null)
            throw new SqlException("Preview requires a DELETE of the selected table with a WHERE condition.");
        BindPreviewTypes(table, delete);
        ValidatePredicate(delete.Where, table.Columns);
        var matches = rows.Where(values => delete.Where.Evaluate(table.Columns.Select((c, i) => (c.Name, Value: values[i]))
            .ToDictionary(pair => pair.Name, pair => pair.Value, StringComparer.OrdinalIgnoreCase)).IsTrue).ToArray();
        if (matches.Length != 1) throw new SqlException("Preview requires exactly one matching row in the loaded result.");
        return matches[0];
    }

    private static void BindPreviewTypes(SqlTable table, SqlStatement statement)
    {
        if (statement.ExpressionBindings is not { } bindings) return;
        foreach (var cast in bindings.NamedCasts)
            cast.DistinctType = table.Columns.Select(c => c.DistinctType).FirstOrDefault(t => string.Equals(t?.Name, cast.Type, StringComparison.OrdinalIgnoreCase))
                ?? throw new SqlException($"Unknown SQL type '{cast.Type}'.");
        if (bindings.Additions.Count > 0) throw new SqlException("Preview cannot resolve custom addition operators.");
    }

    public static void PreviewUpdate(SqlTable table, IReadOnlyList<SqlValue[]> rows, UpdateRows update)
    {
        if (!string.Equals(update.Table, table.Name, StringComparison.OrdinalIgnoreCase) || update.Where is null)
            throw new SqlException("Preview requires an UPDATE of the selected table with a WHERE condition.");
        BindPreviewTypes(table, update);
        ValidatePredicate(update.Where, table.Columns);
        var assignments = ResolveAssignments(table, update.Assignments);
        var matches = rows.Select(values => (Values: values, Fields: table.Columns.Select((column, index) => (column.Name, values[index]))
                .ToDictionary(pair => pair.Name, pair => pair.Item2, StringComparer.OrdinalIgnoreCase)))
            .Where(row => update.Where.Evaluate(row.Fields).IsTrue).ToArray();
        if (matches.Length != 1) throw new SqlException("Preview requires exactly one matching row in the loaded result.");
        var match = matches[0];
        var changed = new Dictionary<string, SqlValue>(match.Fields, StringComparer.OrdinalIgnoreCase);
        ApplyAssignments(table, changed, assignments, match.Fields);
        for (int i = 0; i < table.Columns.Count; i++) match.Values[i] = changed[table.Columns[i].Name];
    }
}
