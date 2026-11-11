using System.Globalization;
using System.Text;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed partial class TableEditSession
{
    public SqlTable Table { get; }
    public string SelectSql { get; }
    public int ConnectionVersion { get; }
    public SqlResult Result { get; private set; }
    public int PendingCount { get; private set; }
    public bool IsCurrent { get; set; } = true;
    public void MarkApplied() { PendingCount = 0; IsCurrent = false; ClearHistory(); }

    public TableEditSession(SqlTable table, SqlResult result, string selectSql, int connectionVersion)
    {
        if (table.ViewQuery is not null || table.Columns.Count == 0 || !table.Columns.Select(c => c.Name).SequenceEqual(result.Columns, StringComparer.OrdinalIgnoreCase))
            throw new InvalidOperationException(L.Get("EditSchemaMismatch"));
        Table = table; SelectSql = selectSql; ConnectionVersion = connectionVersion;
        originalRows = result.Rows.Select(row => (SqlValue[])row.Clone()).ToArray();
        rowIdentities = Enumerable.Range(0, originalRows.Length).ToList();
        Result = result with { Rows = result.Rows.Select(row => (SqlValue[])row.Clone()).ToArray() };
    }

    public SqlValue ParseValue(int column, string text, bool isNull)
    {
        var definition = Table.Columns[column];
        if (isNull)
        {
            if (definition.NotNull || definition.PrimaryKey || Table.CompositePrimaryKey?.Contains(definition.Name, StringComparer.OrdinalIgnoreCase) == true)
                throw new ArgumentException(L.Get("EditNullNotAllowed"));
            return SqlValue.Null;
        }
        string type = definition.DistinctType?.BaseType ?? definition.Type;
        SqlValue input;
        if (type is "INTEGER" or "REAL" || type.StartsWith("DECIMAL(", StringComparison.Ordinal))
        {
            input = SqlValue.Number(text.Trim());
            if (type == "INTEGER" && decimal.Parse(input.Text!, CultureInfo.InvariantCulture) != decimal.Truncate(decimal.Parse(input.Text!, CultureInfo.InvariantCulture)))
                throw new ArgumentException(L.Get("EditIntegerRequired"));
        }
        else input = SqlValue.String(type == "TEXT" ? text.ReplaceLineEndings("\n") : type == "BLOB" && text.StartsWith("0x", StringComparison.OrdinalIgnoreCase) ? text[2..] : text);
        var value = new SqlCast(new SqlLiteral(input), type, definition.MaxLength ?? definition.DistinctType?.MaxLength).Evaluate(new Dictionary<string, SqlValue>());
        return definition.DistinctType is null || value.IsNull ? value : value with { TypeName = definition.DistinctType.Name };
    }

    public string Stage(int row, int column, SqlValue value)
    {
        if (!IsCurrent) throw new InvalidOperationException(L.Get("EditRefreshRequired"));
        if (IsDeleted(row)) throw new InvalidOperationException(L.Get("EditDeletedRow"));
        var values = Result.Rows[row];
        if (values[column] == value) return "";
        var sql = BuildUpdate(values, column, value);
        generated[StatementKey(sql)] = new GeneratedEdit(rowIdentities[row], column, value);
        SqlParser.Parse(sql);
        bool wasModified = IsModified(row, column);
        values[column] = value;
        ModifiedCellCount += (IsModified(row, column) ? 1 : 0) - (wasModified ? 1 : 0);
        PendingCount++;
        return sql;
    }

    private string BuildUpdate(SqlValue[] values, int column, SqlValue value)
    {
        return $"UPDATE {SqlIdentifiers.Format(Table.Name)} SET {SqlIdentifiers.Format(Table.Columns[column].Name)} = {Literal(value)} WHERE {Predicate(values)} RETURNING *;";
    }

    private string Predicate(SqlValue[] values) => string.Join(" AND ", Table.Columns.Select((c, index) =>
        SqlIdentifiers.Format(c.Name) + (values[index].IsNull ? " IS NULL" : " = " + Literal(values[index]))));

    private static string Literal(SqlValue value)
    {
        if (value.IsNull) return "NULL";
        string Quoted(string text)
        {
            if (!text.Any(char.IsControl)) return "'" + text.Replace("'", "''") + "'";
            var literal = new StringBuilder("U&'");
            foreach (char c in text)
            {
                if (char.IsControl(c)) literal.Append('\\').Append(((int)c).ToString("X4", CultureInfo.InvariantCulture));
                else if (c == '\\') literal.Append("\\\\");
                else if (c == '\'') literal.Append("''");
                else literal.Append(c);
            }
            return literal.Append('\'').ToString();
        }
        string raw = value.Kind switch
        {
            "NUMBER" => SqlValue.Number(value.Text!).Text!,
            "BOOLEAN" => value.IsTrue ? "TRUE" : "FALSE",
            "BLOB" => "X'" + Convert.ToHexString(Convert.FromHexString(value.Text!)) + "'",
            "TEXT" => Quoted(value.Text!),
            "UUID" or "DATE" or "TIME" or "LOCAL_DATETIME" or "INSTANT" or "DURATION" => $"CAST({Quoted(value.Text!)} AS {value.Kind})",
            _ => throw new ArgumentException(L.Get("EditUnsupportedType", value.Kind))
        };
        return value.TypeName is null ? raw : $"CAST({raw} AS {SqlIdentifiers.Format(value.TypeName)})";
    }
}

internal static class TableEditBatch
{
    internal static bool IsAllowed(SqlStatement statement, string table) => SqlDatabase.CanPreviewWrite(statement, table);

    public static async Task<bool> ExecuteAsync(string sql, string table,
        Func<string, Task<IReadOnlyList<SqlResult>>> execute, Func<bool> inTransaction)
    {
        var statements = SqlParser.Parse(sql);
        if (statements.Count == 0 || statements.Any(s => !IsAllowed(s, table)))
            throw new InvalidOperationException(L.Get("EditUpdatesOnly"));
        bool own = !inTransaction();
        string savepoint = "owner_edit_" + Guid.NewGuid().ToString("N");
        await execute(own ? "BEGIN;" : $"SAVEPOINT {savepoint};");
        try
        {
            var results = await execute(sql);
            if (results.Count != statements.Count || results.Any(result => result.Rows.Count != 1))
                throw new InvalidOperationException(L.Get("EditAffectedRowMismatch"));
            await execute(own ? "COMMIT;" : $"RELEASE SAVEPOINT {savepoint};");
            return own;
        }
        catch (Exception error)
        {
            if (inTransaction())
            {
                try { await execute(own ? "ROLLBACK;" : $"ROLLBACK TO SAVEPOINT {savepoint}; RELEASE SAVEPOINT {savepoint};"); }
                catch (Exception rollback) { throw new AggregateException(L.Get("EditRollbackFailed"), error, rollback); }
            }
            throw;
        }
    }
}
