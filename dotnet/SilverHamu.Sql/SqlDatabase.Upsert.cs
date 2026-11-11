namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    private sealed record ConflictKey(IReadOnlyList<string> Columns, Func<SqlRow, string?> Identity, bool Partial = false);

    private static IEnumerable<ConflictKey> ConflictKeys(SqlTable table)
    {
        foreach (var column in table.Columns.Where(c => c.Unique))
            yield return new([column.Name], row => ReferenceIdentity(row, [column.Name]));
        if (table.CompositePrimaryKey is { } primary)
            yield return new(primary, row => ReferenceIdentity(row, primary));
        foreach (var index in UniqueIndexes(table))
            yield return new(index.ColumnNames, row => UniqueIndexIdentity(index, row), index.Predicate is not null);
    }

    private async Task<SqlResult> UpsertAsync(SqlTable table, List<SqlRow> existing, InsertStatement insert,
        List<(string Key, byte[]? Value)> changes, List<PendingTrigger> triggers, List<SqlRow>? returned)
    {
        var conflict = insert.Conflict!;
        var rules = ConflictKeys(table);
        if (conflict.Columns is { } target)
        {
            var targetNames = target.Select(name => Column(table, name).Name).ToHashSet(StringComparer.OrdinalIgnoreCase);
            rules = rules.Where(key => !key.Partial && targetNames.SetEquals(key.Columns)).Take(1);
        }
        var indexes = rules.Select(rule => (Rule: rule, Matches: new Dictionary<string, string>(StringComparer.Ordinal))).ToArray();
        if (conflict.Columns is not null && indexes.Length == 0)
            throw new SqlException("ON CONFLICT target must match a PRIMARY KEY or non-partial UNIQUE key.");
        foreach (var row in existing)
            foreach (var (rule, matches) in indexes)
                if (rule.Identity(row) is { } key) matches.Add(key, row.StorageKey);

        var originalColumns = InsertColumns(table, insert.Columns);
        var schema = table.Columns.Concat(table.Columns.Select(c => c with { Name = ExcludedColumnKey(c) })).ToArray();
        SqlExpression Bind(SqlExpression expression) => expression.Rewrite(node =>
        {
            if (node is SqlNestedQuery or SqlAggregate) throw new SqlException("ON CONFLICT requires scalar expressions.");
            if (node is not SqlField field) return null;
            var column = Column(table, field.Name);
            if (field.Qualifier is null || SameTable(field.Qualifier, table.Name)) return new SqlField(column.Name);
            if (SameTable(field.Qualifier, "excluded")) return new SqlField(ExcludedColumnKey(column));
            throw new SqlException($"Unknown qualifier '{field.Qualifier}' in ON CONFLICT.");
        });
        if (conflict.Assignments is not null && SameTable(table.Name, "excluded"))
            throw new SqlException("The target name collides with the excluded row.");
        var assignments = conflict.Assignments?.Select(pair => new Assignment(Column(table, pair.Key), Bind(pair.Value))).ToArray();
        foreach (var assignment in assignments ?? []) ValidateAssignment(assignment.Column, assignment.Expression, schema);
        var predicate = conflict.Where is null ? null : Bind(conflict.Where);
        ValidatePredicate(predicate, schema);
        var proposed = existing.ToDictionary(row => row.StorageKey);
        var touched = new HashSet<string>(StringComparer.Ordinal);
        int count = 0;
        var inputs = InsertValuesForColumns(insert, originalColumns)
            .Select(values => MakeInsertedRow(table, originalColumns, values)).ToList();
        await ValidateChecksAsync(table, inputs);
        foreach (var input in inputs)
        {
            var hit = indexes.Select(index => index.Rule.Identity(input) is { } key ? index.Matches.GetValueOrDefault(key) : null)
                .FirstOrDefault(key => key is not null);
            SqlRow next;
            if (hit is null)
            {
                if (proposed.Count == MaxRows) throw new SqlException($"1テーブルの上限は{MaxRows}行です。");
                next = input;
            }
            else
            {
                if (assignments is null) continue;
                var old = proposed[hit];
                var scope = new Dictionary<string, SqlValue>(old.Values, StringComparer.OrdinalIgnoreCase);
                foreach (var c in table.Columns) scope.Add(ExcludedColumnKey(c), input.Values[c.Name]);
                if (!Matches(predicate, scope)) continue;
                var updated = new Dictionary<string, SqlValue>(old.Values, StringComparer.OrdinalIgnoreCase);
                ApplyAssignments(table, updated, assignments, scope);
                next = new(hit, updated);
                foreach (var (rule, matches) in indexes)
                    if (rule.Identity(old) is { } oldKey) matches.Remove(oldKey);
            }
            if (!touched.Add(next.StorageKey)) throw new SqlException("ON CONFLICT cannot affect the same row twice.");
            foreach (var (rule, matches) in indexes)
                if (rule.Identity(next) is { } key && !matches.TryAdd(key, next.StorageKey))
                    throw new SqlException("ON CONFLICT update violates a unique constraint.");
            proposed[next.StorageKey] = next;
            returned?.Add(next);
            count++;
        }
        await ApplyReferentialActionsAsync(table, existing, proposed.Values.ToList(), changes, triggers);
        return Empty($"ON CONFLICT で {count} 行を変更しました。");
    }

    // SQL identifiers cannot contain control characters, so quoted column names cannot shadow this key.
    private static string ExcludedColumnKey(SqlColumn column) => "\0excluded:" + column.Name;

    private static SqlResult ReturnRows(SqlTable table, List<SqlProjection> returning, List<SqlRow> rows, SqlResult status)
    {
        var query = new SelectRows(table.Name, returning, null, [], null, 0);
        return Select(BindSingleSource(query, table), table, rows) with { Message = status.Message };
    }
}
