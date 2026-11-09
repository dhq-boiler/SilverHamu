namespace Catdb.Sql;

internal sealed record SqlInSubquery(SqlField Field, SelectRows Query) : SqlExpression
{
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) =>
        throw new InvalidOperationException("Unbound IN subquery.");
}

internal sealed record SqlInValues(SqlField Field, string InnerType, HashSet<string> Values, bool ContainsNull) : SqlExpression
{
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var value = Field.Evaluate(row);
        if (Values.Count == 0 && !ContainsNull) return SqlValue.Boolean(false);
        if (value.IsNull) return SqlValue.Null;
        if (Values.Contains(SqlDatabase.UniqueIdentity(value))) return SqlValue.Boolean(true);
        return ContainsNull ? SqlValue.Null : SqlValue.Boolean(false);
    }
}

public sealed partial class SqlDatabase
{
    private async Task<SqlInValues> EvaluateSubqueryAsync(SqlInSubquery predicate)
    {
        var query = predicate.Query;
        var name = query.Table!;
        var tableBytes = await GetAsync(TableKey(name));
        SqlTable source;
        SqlResult selected;
        if (tableBytes is not null)
        {
            source = Deserialize<SqlTable>(tableBytes);
            selected = await SelectTableAsync(query, source);
        }
        else
        {
            var viewBytes = await GetAsync(ViewKey(name)) ?? throw new SqlException($"参照元 '{name}' は存在しません。");
            source = Deserialize<SqlTable>(viewBytes);
            selected = await SelectViewAsync(query, source);
        }
        return BindSubqueryValues(predicate, source, selected);
    }

    private static SqlInValues BindSubqueryValues(SqlInSubquery predicate, SqlTable source, SqlResult selected)
    {
        var field = (SqlField)predicate.Query.Projections[0].Expression!;
        var innerType = ComparisonType(Column(source, field.Name));
        var values = new HashSet<string>(StringComparer.Ordinal);
        bool containsNull = false;
        foreach (var row in selected.Rows)
        {
            if (row[0].IsNull) containsNull = true;
            else values.Add(UniqueIdentity(row[0]));
        }
        return new(predicate.Field, innerType, values, containsNull);
    }
}
