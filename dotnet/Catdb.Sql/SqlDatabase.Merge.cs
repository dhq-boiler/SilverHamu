namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed partial class SelectScope
    {
        public SelectScope() { }
        public List<SqlColumn> MergeColumns() => columns.Select(c => c.Column with { Name = c.Key }).ToList();
    }

    private sealed record BoundMergeClause(bool Matched, SqlExpression? Condition,
        Func<SqlRow?, Dictionary<string, SqlValue>, SqlRow?> Change);

    private async Task<SqlResult> MergeAsync(MergeRows merge, List<(string Key, byte[]? Value)> changes)
    {
        var target = await ReadSourceTableAsync(merge.Target);
        var original = await LoadRowsAsync(target);
        var (source, input) = await LoadMergeSourceAsync(merge.Source);
        var scope = new SelectScope();
        scope.AddSource(target, merge.Alias, 0, true);
        scope.AddSource(source, merge.Source.Alias, 1, true);
        var sourceOnly = new SelectScope();
        sourceOnly.AddSource(source, merge.Source.Alias, 1, true);

        var on = BindMergeExpression(scope, merge.On);
        RequireType(ExpressionType(on, scope.MergeColumns()), "BOOLEAN");
        var clauses = merge.Clauses.Select(clause =>
            BindMergeClause(target, clause.Matched ? scope : sourceOnly, clause)).ToArray();
        var candidates = MergeCandidates(merge.On, scope, original);
        var changedKeys = new HashSet<string>(StringComparer.Ordinal);
        var proposed = original.ToDictionary(r => r.StorageKey, StringComparer.Ordinal);
        var context = new Dictionary<string, SqlValue>(StringComparer.OrdinalIgnoreCase);
        var targetKeys = target.Columns.Select(c => (c.Name, Key: "0:" + c.Name)).ToArray();
        var sourceKeys = source.Columns.Select(c => (c.Name, Key: "1:" + c.Name)).ToArray();

        int affected = 0;
        foreach (var row in input)
        {
            foreach (var (name, key) in sourceKeys) context[key] = row.Values[name];
            bool matched = false;
            foreach (var existing in candidates(row))
            {
                foreach (var (name, key) in targetKeys) context[key] = existing.Values[name];
                if (!Matches(on, context)) continue;
                matched = true;
                var clause = clauses.FirstOrDefault(c => c.Matched && Matches(c.Condition, context));
                if (clause is null) continue;
                if (!changedKeys.Add(existing.StorageKey))
                    throw new SqlException("MERGE の複数の入力行が同じ対象行を変更しようとしました。");
                var replacement = clause.Change(existing, context);
                if (replacement is null) proposed.Remove(existing.StorageKey);
                else proposed[existing.StorageKey] = replacement;
                affected++;
            }
            if (matched) continue;
            var insert = clauses.FirstOrDefault(c => !c.Matched && Matches(c.Condition, context));
            if (insert is null) continue;
            var inserted = insert.Change(null, context)!;
            proposed.Add(inserted.StorageKey, inserted);
            affected++;
        }
        if (proposed.Count > MaxRows) throw new SqlException($"1テーブルの上限は{MaxRows}行です。");
        await ApplyReferentialActionsAsync(target, original, proposed.Values.ToList(), changes);
        return Empty($"MERGE で {affected} 行を変更しました。");
    }

    // Candidate rows always come from the state before the MERGE. For the common
    // equality predicate, build the lookup once, including for a self-merge.
    private static Func<SqlRow, IEnumerable<SqlRow>> MergeCandidates(SqlExpression on, SelectScope scope, List<SqlRow> original)
    {
        if (on is SqlBinary { Operator: "=", Left: SqlField left, Right: SqlField right })
        {
            var first = scope.Resolve(left);
            var second = scope.Resolve(right);
            if (first.Source != second.Source)
            {
                var (target, input) = first.Source == 0 ? (first.Column, second.Column) : (second.Column, first.Column);
                if (target.Type != "NULL" && input.Type != "NULL" && ComparisonType(target) != ComparisonType(input))
                    throw new SqlException("MERGE の ON で指定した列の型が一致しません。");
                var lookup = original.Where(r => !r.Values[target.Name].IsNull)
                    .ToLookup(r => UniqueIdentity(r.Values[target.Name]), StringComparer.Ordinal);
                return row => row.Values[input.Name] is { IsNull: false } value ? lookup[UniqueIdentity(value)] : [];
            }
        }
        return _ => original;
    }

    private static SqlExpression BindMergeExpression(SelectScope scope, SqlExpression expression) =>
        expression.Rewrite(node => node switch
        {
            SqlField field => new SqlField(scope.Resolve(field).Key),
            SqlNestedQuery or SqlAggregate => throw new SqlException("MERGE の条件と値にはスカラー式を指定してください。"),
            _ => null
        });

    private static BoundMergeClause BindMergeClause(SqlTable table, SelectScope scope, MergeWhen clause)
    {
        var condition = clause.Condition is null ? null : BindMergeExpression(scope, clause.Condition);
        var schema = scope.MergeColumns();
        if (condition is not null) RequireType(ExpressionType(condition, schema), "BOOLEAN");
        switch (clause.Action)
        {
            case MergeUpdate update:
                var assignments = update.Assignments.Select(p => new Assignment(Column(table, p.Key), BindMergeExpression(scope, p.Value))).ToArray();
                foreach (var assignment in assignments) ValidateAssignment(assignment.Column, assignment.Expression, schema);
                return new(true, condition, (original, values) =>
                {
                    var row = new Dictionary<string, SqlValue>(original!.Values, StringComparer.OrdinalIgnoreCase);
                    ApplyAssignments(table, row, assignments, values);
                    return new(original.StorageKey, row);
                });
            case MergeInsert insert:
                var columns = InsertColumns(table, insert.Columns);
                var expressions = insert.Values.Select(e => BindMergeExpression(scope, e)).ToArray();
                if (columns.Count != expressions.Length) throw new SqlException("MERGE の列数と入力の列数が一致しません。");
                for (int i = 0; i < columns.Count; i++) ValidateAssignment(columns[i], expressions[i], schema);
                return new(false, condition, (_, values) => MakeInsertedRow(table, columns, expressions.Select(e => e.Evaluate(values)).ToArray()));
            case MergeDelete:
                return new(true, condition, (_, _) => null);
            default: throw new SqlException("無効な MERGE アクションです。");
        }
    }

    private async Task<(SqlTable, List<SqlRow>)> LoadMergeSourceAsync(MergeSource source)
    {
        SqlResult result;
        if (source.Table is { } name)
        {
            var (table, rows) = await LoadJoinSourceAsync(name, false);
            if (source.Columns is null) return (table, rows);
            result = new(table.Columns.Select(c => c.Name).ToArray(),
                rows.Select(r => table.Columns.Select(c => r.Values[c.Name]).ToArray()).ToArray(), "")
                { ColumnTypes = table.Columns.Select(c => c.Type).ToArray() };
        }
        else if (source.Query is not null) result = await ReadWriteSourceAsync(source.Query);
        else result = MergeValuesSource(source.Values!);
        var schema = TableFromResult(source.Alias, result, source.Columns);
        return (schema, RowsFromResult(schema, result));
    }

    private static SqlResult MergeValuesSource(List<List<SqlExpression>> values)
    {
        var count = values[0].Count;
        var names = Enumerable.Range(1, count).Select(i => $"column{i}").ToArray();
        var types = Enumerable.Repeat("NULL", count).ToArray();
        var rows = new List<SqlValue[]>();
        var empty = new Dictionary<string, SqlValue>();
        foreach (var tuple in values)
        {
            if (tuple.Count != count) throw new SqlException("VALUES の各行の列数が一致しません。");
            var row = new SqlValue[count];
            for (int i = 0; i < count; i++)
            {
                tuple[i].Validate([]);
                types[i] = CommonType(types[i], ExpressionType(tuple[i], []));
                row[i] = tuple[i].Evaluate(empty);
            }
            rows.Add(row);
        }
        return new(names, rows, "") { ColumnTypes = types };
    }
}
