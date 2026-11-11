using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed partial class TableEditSession
{
    private List<int> rowIdentities;
    private HashSet<int> deletedIdentities = [];
    private int nextInsertedIdentity = -1;
    public int AddedRowCount => rowIdentities.Count(id => id < 0 && !deletedIdentities.Contains(id));
    public int DeletedRowCount => deletedIdentities.Count;
    public int IncompleteRowCount { get; private set; }
    public bool IsAdded(int row) => rowIdentities[row] < 0;
    public bool IsDeleted(int row) => deletedIdentities.Contains(rowIdentities[row]);
    public string RowLabel(int row) => (IsDeleted(row) ? "−" : "") + (IsAdded(row) ? "+" + -rowIdentities[row] : (rowIdentities[row] + 1).ToString());
    public string DisplayValue(int row, int column) => IsAdded(row) && Result.Rows[row][column].IsNull && !CanBeNull(column)
        ? L.Get("EditRequiredValue") : Result.Rows[row][column].Display;
    public bool CanBeNull(int column) => !Table.Columns[column].NotNull && !Table.Columns[column].PrimaryKey && Table.CompositePrimaryKey?.Contains(Table.Columns[column].Name, StringComparer.OrdinalIgnoreCase) != true;
    public bool IsUniqueColumn(int column)
    {
        var definition = Table.Columns[column];
        return definition.PrimaryKey || definition.Unique
            || Table.CompositePrimaryKey?.Contains(definition.Name, StringComparer.OrdinalIgnoreCase) == true
            || Table.Indexes?.Any(index => index.Unique && index.ColumnNames.Contains(definition.Name, StringComparer.OrdinalIgnoreCase)) == true;
    }

    private string BuildInsert(int identity, SqlValue[] values) => $"INSERT /* +{-identity} */ INTO {SqlIdentifiers.Format(Table.Name)} ({string.Join(", ", Table.Columns.Select(c => SqlIdentifiers.Format(c.Name)))}) VALUES ({string.Join(", ", values.Select(Literal))}) RETURNING *;";
    private string BuildDelete(SqlValue[] values) => $"DELETE FROM {SqlIdentifiers.Format(Table.Name)} WHERE {Predicate(values)} RETURNING *;";

    private static string Append(string sql, string statement) => sql.TrimEnd() + (string.IsNullOrWhiteSpace(sql) ? "" : "\n") + statement;

    public string AddRow()
    {
        RequirePreview();
        int identity = nextInsertedIdentity--;
        var values = Table.Columns.Select(c => c.Default ?? SqlValue.Null).ToArray();
        string sql = BuildInsert(identity, values);
        generated[StatementKey(sql)] = new GeneratedInsert(identity, values);
        return Synchronize(Append(Script, sql));
    }

    public string DuplicateRows(IEnumerable<int> indices)
    {
        RequirePreview();
        var values = indices.Distinct().Order().Where(row => !IsDeleted(row))
            .Select(row => (SqlValue[])Result.Rows[row].Clone()).ToArray();
        if (values.Length == 0) return Script;
        string sql = Script;
        foreach (var row in values)
        {
            for (int column = 0; column < row.Length; column++)
                if (IsUniqueColumn(column)) row[column] = SqlValue.Null;
            int identity = nextInsertedIdentity--;
            string statement = BuildInsert(identity, row);
            generated[StatementKey(statement)] = new GeneratedInsert(identity, row);
            sql = Append(sql, statement);
        }
        return Synchronize(sql);
    }

    public string ChangeInsertedCell(int row, int column, SqlValue value)
    {
        RequirePreview();
        if (!IsAdded(row) || IsDeleted(row)) throw new InvalidOperationException(L.Get("EditDeletedRow"));
        int identity = rowIdentities[row];
        var part = SqlParser.ParseWithSource(Script).Single(p =>
            generated.TryGetValue(PartKey(p, Script), out var operation) && operation is GeneratedInsert insert && insert.Row == identity);
        var values = (SqlValue[])Result.Rows[row].Clone(); values[column] = value;
        string replacement = BuildInsert(identity, values);
        generated[StatementKey(replacement)] = new GeneratedInsert(identity, values);
        return Synchronize(Script.Remove(part.Start, part.Length).Insert(part.Start, part.Length > 0 ? replacement.TrimEnd(';') : replacement));
    }

    public string DeleteRows(IEnumerable<int> indices)
    {
        RequirePreview();
        var selected = indices.Distinct().Where(row => !IsDeleted(row)).ToArray();
        if (selected.Length == 0) return Script;
        var inserted = selected.Where(IsAdded).Select(row => rowIdentities[row]).ToHashSet();
        string sql = Script;
        foreach (var part in SqlParser.ParseWithSource(Script).Reverse())
            if (generated.TryGetValue(PartKey(part, Script), out var operation) && operation is GeneratedInsert && inserted.Contains(operation.Row))
                sql = sql.Remove(part.Start, part.Length + (part.Start + part.Length < sql.Length && sql[part.Start + part.Length] == ';' ? 1 : 0));
        foreach (int row in selected.Where(row => !IsAdded(row)))
        {
            string statement = BuildDelete(Result.Rows[row]);
            generated[StatementKey(statement)] = new GeneratedDelete(rowIdentities[row]);
            sql = Append(sql, statement);
        }
        return Synchronize(sql);
    }

    private void RequirePreview()
    {
        if (!IsCurrent) throw new InvalidOperationException(L.Get("EditRefreshRequired"));
        if (PreviewError is { } error) throw new InvalidOperationException(L.Get("EditPreviewInvalid", error));
    }
}
