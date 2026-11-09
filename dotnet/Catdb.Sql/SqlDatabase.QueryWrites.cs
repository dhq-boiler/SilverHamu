namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    // A query feeding a write must surface overflow instead of using the client's
    // implicit first page. Keep explicit FETCH/LIMIT clauses in the source.
    private async Task<SqlResult> ReadWriteSourceAsync(SelectRows query)
    {
        var result = await ExecuteStatementAsync(query with { Limit = query.Limit ?? MaxRows + 1 });
        if (result.Rows.Count > MaxRows) throw new SqlException($"A write source exceeds {MaxRows} rows.");
        return result;
    }

    private static string[] ProjectionTypes(IEnumerable<SqlProjection> projections, SqlTable table) =>
        projections.Select(p => p.Window is { } window ? WindowResultType(window, table.Columns) :
            p.Count ? "INTEGER" : p.Aggregate.HasValue ? "REAL" : ExpressionType(p.Expression!, table.Columns)).ToArray();

    private static SqlTable TableFromResult(string name, SqlResult result, IReadOnlyList<string>? aliases = null)
    {
        var names = aliases ?? result.Columns;
        if (names.Count != result.Columns.Count || names.Count is 0 or > 64)
            throw new SqlException("列名の数とクエリの結果列数が一致しません。");
        if (names.Distinct(StringComparer.OrdinalIgnoreCase).Count() != names.Count)
            throw new SqlException("結果の列名が重複しています。別名を指定してください。");
        var types = ResultColumnTypes(result);
        return new(name, names.Select((column, i) => new SqlColumn(column, types[i], false, false, false, null, null)).ToList());
    }

    private static IReadOnlyList<string> ResultColumnTypes(SqlResult result) =>
        result.ColumnTypes is { } types && types.Count == result.Columns.Count
            ? types : throw new SqlException("クエリの結果列の型を特定できません。");

    private static List<SqlRow> RowsFromResult(SqlTable table, SqlResult result) =>
        result.Rows.Select(row => new SqlRow("", table.Columns.Select((column, i) => KeyValuePair.Create(column.Name, row[i]))
            .ToDictionary(StringComparer.OrdinalIgnoreCase))).ToList();

    private async Task<bool> CanCreateTableAsync(string name, bool ifNotExists)
    {
        if (await GetAsync(ViewKey(name)) is not null) throw new SqlException("同名のビューがあります。");
        if (await GetAsync(TableKey(name)) is not null)
        {
            if (!ifNotExists) throw new SqlException($"テーブル '{name}' は既に存在します。");
            return false;
        }
        if ((await GetTablesAsync()).Count >= 256) throw new SqlException("テーブル数の上限256に達しています。");
        return true;
    }

    private async Task<SqlResult> CreateTableAsAsync(CreateTableAs create, List<(string Key, byte[]? Value)> changes)
    {
        if (!await CanCreateTableAsync(create.Table, create.IfNotExists)) return Empty("テーブルは既に存在します。変更なし。");
        var input = await ReadWriteSourceAsync(create.Query);
        var table = TableFromResult(create.Table, input, create.Columns);
        // An untyped NULL in a standalone projection has no source column declaration.
        var columns = table.Columns.Select(c => c.Type == "NULL" ? c with { Type = "TEXT" } : c);
        table = table with { Columns = await ResolveColumnTypesAsync(columns) };
        changes.Add((TableKey(table.Name), Serialize(table)));
        foreach (var values in input.Rows)
        {
            var row = MakeInsertedRow(table, table.Columns, values);
            changes.Add((row.StorageKey, Serialize(row.Values)));
        }
        return Empty($"テーブル '{table.Name}' を作成し、{input.Rows.Count} 行を挿入しました。");
    }

    private static List<SqlColumn> InsertColumns(SqlTable table, List<string>? names) =>
        names is null ? table.Columns : names.Select(name => Column(table, name)).ToList();

    private static void ValidateInsertTypes(IReadOnlyList<SqlColumn> columns, IReadOnlyList<string> types)
    {
        if (columns.Count != types.Count) throw new SqlException("INSERT の列数と入力の列数が一致しません。");
        for (int i = 0; i < columns.Count; i++) CommonType(columns[i].Type, types[i]);
    }

    private static IEnumerable<IReadOnlyList<SqlValue>> InsertValuesForColumns(InsertStatement insert, IReadOnlyList<SqlColumn> columns)
    {
        if (insert is InsertRows literals)
        {
            if (literals.SourceTypes is { } types) ValidateInsertTypes(columns, types);
            return literals.Rows;
        }
        var empty = new Dictionary<string, SqlValue>();
        return ((InsertValues)insert).Rows.Select(row =>
        {
            if (row.Count != columns.Count) throw new SqlException("INSERT の列数と入力の列数が一致しません。");
            for (var i = 0; i < row.Count; i++)
                if (row[i] is SqlCast cast) CommonType(columns[i].Type, ExpressionType(cast, []));
            return row.Select(expression => expression.Evaluate(empty)).ToArray();
        });
    }

    private static SqlRow MakeInsertedRow(SqlTable table, IReadOnlyList<SqlColumn> columns, IReadOnlyList<SqlValue> values)
    {
        if (values.Count != columns.Count) throw new SqlException("INSERT の列数と値の数が一致しません。");
        var row = table.Columns.ToDictionary(c => c.Name, c => c.Default ?? SqlValue.Null, StringComparer.OrdinalIgnoreCase);
        for (int i = 0; i < columns.Count; i++) row[columns[i].Name] = values[i];
        ValidateRow(table, row);
        return new(RowPrefix(table.Name) + Guid.NewGuid().ToString("N"), row);
    }

    private sealed record Assignment(SqlColumn Column, SqlExpression Expression);

    private static void ValidateAssignment(SqlColumn column, SqlExpression expression, IReadOnlyList<SqlColumn> schema)
    {
        if (expression is SqlLiteral literal) Coerce(column, literal.Value);
        else CommonType(column.Type, ExpressionType(expression, schema));
    }

    private static Assignment[] ResolveAssignments(SqlTable table, IReadOnlyDictionary<string, SqlExpression> values)
    {
        return values.Select(pair =>
        {
            var column = Column(table, pair.Key);
            var expression = pair.Value.Rewrite(node => node is SqlField { Qualifier: { } qualifier } field &&
                SameTable(qualifier, table.Name) ? field with { Qualifier = null } : null);
            expression.Validate(table.Columns);
            if (expression.ContainsAggregate()) throw new SqlException("UPDATE の SET に集約関数を指定できません。");
            ValidateAssignment(column, expression, table.Columns);
            return new Assignment(column, expression);
        }).ToArray();
    }

    private static void ApplyAssignments(SqlTable table, Dictionary<string, SqlValue> destination, Assignment[] assignments,
        IReadOnlyDictionary<string, SqlValue> original)
    {
        foreach (var assignment in assignments)
            destination[assignment.Column.Name] = Coerce(assignment.Column, assignment.Expression.Evaluate(original));
        ValidateRow(table, destination);
    }
}
