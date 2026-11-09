namespace Catdb.Sql;

public sealed partial record SelectRows
{
    internal SqlDatabase.QueryBinding? Binding { get; init; }
}

public sealed partial class SqlDatabase
{
    internal sealed record QueryBinding(
        Func<SelectRows, Func<SqlField, SqlExpression?>, bool, SqlResult> Run,
        Func<SqlField, SqlExpression?>? Outer);

    private sealed partial class SelectScope
    {
        private bool IsLocal(SqlField field) => field.Qualifier is null
            ? visible.Any(c => Same(c.Column.Name, field.Name)) : qualifiers.Contains(field.Qualifier);

        private SqlExpression BindField(SqlField field)
        {
            if (!IsLocal(field) && queryBinding?.Outer?.Invoke(field) is { } outer) return outer;
            return new SqlField(SelectKey(Resolve(field)));
        }

        private SqlExpression BindNestedQuery(SqlNestedQuery nested)
        {
            if (queryBinding is null) throw new SqlException("This SQL context does not permit a subquery.");
            var capture = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            SqlExpression? Capture(SqlField field, IReadOnlyDictionary<string, SqlValue>? row)
            {
                if (!IsLocal(field)) return queryBinding.Outer?.Invoke(field);
                var column = Resolve(field);
                var key = SelectKey(column);
                capture.Add(key);
                return new SqlBoundQuery(column.Column.Type, _ => row is null ? SqlValue.Null : row[key]);
            }

            var shape = queryBinding.Run(nested.Query, field => Capture(field, null), true);
            if (nested.Kind != SubqueryKind.Exists && shape.Columns.Count != 1)
                throw new SqlException("A scalar or IN subquery must return exactly one column.");
            var type = shape.ColumnTypes?[0] ?? "NULL";
            var operand = BindExpression(nested.Operand);
            if (operand is not null)
            {
                var schema = columns.Concat(merged).Select(c => c.Column with { Name = c.Key }).ToList();
                _ = CommonType(type, ExpressionType(operand, schema));
            }
            // Cache uncorrelated queries within this invocation of the enclosing query.
            var execution = nested.Query;
            if (nested.Kind == SubqueryKind.Exists && execution.SetExpression is null && !execution.IsGrouped &&
                (!execution.Distinct || execution.Offset == 0))
                execution = execution with
                {
                    Projections = [new(new SqlLiteral(SqlValue.Boolean(true)), "exists")],
                    Distinct = false, Order = [], Limit = Math.Min(execution.Limit ?? 1, 1)
                };
            SqlResult? cached = null;
            HashSet<string>? members = null;
            bool hasNull = false;
            bool correlated = capture.Count > 0;
            var dependencies = new HashSet<string>(capture, StringComparer.OrdinalIgnoreCase);
            operand?.Rewrite(node =>
            {
                if (node is SqlField f) dependencies.Add(f.Name);
                if (node is SqlBoundQuery sub) dependencies.UnionWith(sub.CorrelatedFields);
                return null;
            });
            SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
            {
                var result = cached ?? queryBinding.Run(execution, field => Capture(field, row), false);
                if (!correlated) cached = result;
                if (nested.Kind == SubqueryKind.Exists) return SqlValue.Boolean(result.Rows.Count != 0);
                if (nested.Kind == SubqueryKind.Scalar)
                    return result.Rows.Count switch
                    {
                        0 => SqlValue.Null, 1 => result.Rows[0][0],
                        _ => throw new SqlException("A scalar subquery returned multiple rows.")
                    };
                var value = operand!.Evaluate(row);
                if (!correlated)
                {
                    if (members is null)
                    {
                        members = new(StringComparer.Ordinal);
                        foreach (var values in result.Rows)
                            if (values[0].IsNull) hasNull = true; else members.Add(UniqueIdentity(values[0]));
                    }
                    if (!value.IsNull && members.Contains(UniqueIdentity(value))) return SqlValue.Boolean(true);
                    return hasNull || value.IsNull && members.Count > 0 ? SqlValue.Null : SqlValue.Boolean(false);
                }
                bool unknown = false;
                foreach (var values in result.Rows)
                {
                    if (value.IsNull || values[0].IsNull) unknown = true;
                    else if (SqlValue.Compare(value, values[0]) == 0) return SqlValue.Boolean(true);
                }
                return unknown ? SqlValue.Null : SqlValue.Boolean(false);
            }
            return new SqlBoundQuery(nested.Kind == SubqueryKind.Scalar ? type : "BOOLEAN", Evaluate)
                { CorrelatedFields = dependencies.ToArray() };
        }
    }

    private sealed record Relation(SqlTable Table, List<SqlRow> Rows);
    private static readonly StringComparer SqlNameComparer = StringComparer.OrdinalIgnoreCase;

    // Only dependencies of this query are loaded. Lexically scoped CTE names shadow physical tables.
    private static IEnumerable<string> RequiredSources(SelectRows query, HashSet<string> inherited)
    {
        var visible = new HashSet<string>(inherited, SqlNameComparer);
        foreach (var cte in query.CommonTables)
        {
            if (query.Recursive) visible.Add(cte.Name);
            foreach (var source in RequiredSources(cte.Query, visible)) yield return source;
            visible.Add(cte.Name);
        }
        if (query.SetExpression is { } set)
        {
            foreach (var name in RequiredSources(set.Left, visible)) yield return name;
            foreach (var name in RequiredSources(set.Right, visible)) yield return name;
        }
        if (query.Table is not null)
            foreach (var table in query.Joins().Select(j => j.Table).Prepend(query.Table))
            {
                if (query.DerivedSources?.TryGetValue(table, out var derived) == true)
                {
                    foreach (var source in RequiredSources(derived.Query, visible)) yield return source;
                }
                else if (!visible.Contains(table)) yield return table;
            }
        var nested = new List<SelectRows>();
        foreach (var expr in query.Expressions())
            expr?.Rewrite(node => { if (node is SqlNestedQuery n) nested.Add(n.Query); return null; });
        foreach (var sub in nested)
            foreach (var name in RequiredSources(sub, visible)) yield return name;
    }

    private static async Task<SqlResult> QueryRelationsAsync(SelectRows query,
        Func<string, Task<(SqlTable Table, List<SqlRow> Rows)>> load)
    {
        var tables = new Dictionary<string, Relation>(SqlNameComparer);
        foreach (var name in RequiredSources(query, new(SqlNameComparer)).Distinct(SqlNameComparer))
        {
            var source = await load(name);
            tables.Add(name, new(source.Table, source.Rows));
        }
        return RunQuery(query, tables, null, false, finalPage: true);
    }

    private static Relation MakeRelation(string name, SqlResult result, IReadOnlyList<string>? aliases = null)
    {
        var schema = TableFromResult(name, result, aliases);
        return new(schema, RowsFromResult(schema, result));
    }

    private static SqlResult RunQuery(SelectRows query, Dictionary<string, Relation> inherited,
        Func<SqlField, SqlExpression?>? outer, bool describe, bool finalPage = false)
    {
        // Only the client-facing result has an implicit page size. For a relation consumed by
        // another clause, probe one extra row so a bounded query never silently changes its data.
        describe |= query.Limit == 0;
        query = query with { Limit = describe ? 0 : query.Limit ?? (finalPage ? MaxRows : MaxRows + 1) };
        var result = EvaluateQuery(query, inherited, outer, describe);
        if (result.Rows.Count > MaxRows)
            throw new SqlException($"An intermediate query result exceeds {MaxRows} rows.");
        return result;
    }

    private static SqlResult EvaluateQuery(SelectRows query, Dictionary<string, Relation> inherited,
        Func<SqlField, SqlExpression?>? outer, bool describe)
    {
        var scope = inherited;
        if (query.CommonTables.Count > 0 || query.DerivedSources?.Count > 0)
            scope = new(inherited, SqlNameComparer);
        foreach (var cte in query.CommonTables)
        {
            var selfReference = query.Recursive && ReadsTable(cte.Query, cte.Name);
            var result = selfReference ? RecursiveCte(cte, scope, outer, describe) :
                RunQuery(cte.Query, scope, outer, describe);
            scope[cte.Name] = MakeRelation(cte.Name, result, cte.Columns);
        }
        if (query.SetExpression is { } set)
            return SetResult(query, set, scope, outer, describe);
        if (query.DerivedSources is not null)
            foreach (var derived in query.DerivedSources)
                scope[derived.Key] = MakeRelation(derived.Key, RunQuery(derived.Value.Query, scope, outer, describe), derived.Value.Columns);
        query = query with
        {
            Binding = new((nested, frame, shapeOnly) => RunQuery(nested, scope, frame, shapeOnly), outer)
        };
        if (query.Table is null)
        {
            var unit = new SqlTable("", []);
            return Select(BindSingleSource(query, unit), unit, describe ? [] : [new("", new(SqlNameComparer))], describe);
        }
        if (query.Join is not null)
        {
            var sources = new List<(SqlTable Table, List<SqlRow> Rows)>();
            foreach (var name in query.Joins().Select(j => j.Table).Prepend(query.Table))
            {
                var source = scope[name];
                sources.Add((source.Table, describe ? [] : source.Rows));
            }
            return SelectJoined(query, sources, describe);
        }
        var table = scope[query.Table];
        return Select(BindSingleSource(query, table.Table), table.Table, describe ? [] : table.Rows, describe);
    }

    private static bool ReadsTable(SelectRows query, string name) =>
        RequiredSources(query, new(SqlNameComparer)).Contains(name, SqlNameComparer);

    private static SqlResult RecursiveCte(SqlCommonTable cte, Dictionary<string, Relation> scope,
        Func<SqlField, SqlExpression?>? outer, bool describe)
    {
        var body = cte.Query;
        if (body.SetExpression is not { Operation: "UNION" or "UNION ALL" } union ||
            ReadsTable(union.Left, cte.Name) || !ReadsTable(union.Right, cte.Name))
            throw new SqlException("A recursive CTE requires a nonrecursive anchor UNION [ALL] a recursive term.");
        if (body.CommonTables.Count != 0 || body.Order.Count != 0 || body.Limit.HasValue || body.Offset != 0)
            throw new SqlException("Place ordering and pagination in the query reading the recursive CTE.");
        var anchor = RunQuery(union.Left, scope, outer, describe);
        var seen = union.Operation == "UNION" ? new HashSet<SqlValue[]>(new GroupKeyComparer()) : null;
        var rows = anchor.Rows.Where(row => seen?.Add(row) != false).ToList();
        var delta = (IReadOnlyList<SqlValue[]>)rows.ToArray();
        var local = new Dictionary<string, Relation>(scope, SqlNameComparer);
        var types = anchor.ColumnTypes;
        do
        {
            local[cte.Name] = MakeRelation(cte.Name, anchor with { Rows = delta, ColumnTypes = types }, cte.Columns);
            var step = RunQuery(union.Right, local, outer, describe || delta.Count == 0);
            types = SetTypes(anchor with { ColumnTypes = types }, step);
            delta = step.Rows.Where(row => seen?.Add(row) != false).ToArray();
            if (rows.Count + delta.Count > MaxRows)
                throw new SqlException($"A recursive CTE may produce at most {MaxRows} rows.");
            rows.AddRange(delta);
        } while (!describe && delta.Count > 0);
        return anchor with { Rows = rows, ColumnTypes = types };
    }

    private static IReadOnlyList<string> SetTypes(SqlResult left, SqlResult right)
    {
        if (left.Columns.Count != right.Columns.Count)
            throw new SqlException("Set operands must return the same number of columns.");
        return Enumerable.Range(0, left.Columns.Count)
            .Select(i => CommonType(left.ColumnTypes?[i] ?? "NULL", right.ColumnTypes?[i] ?? "NULL")).ToArray();
    }

    private static SqlResult SetResult(SelectRows query, SqlSetExpression set, Dictionary<string, Relation> scope,
        Func<SqlField, SqlExpression?>? outer, bool describe)
    {
        var left = RunQuery(set.Left, scope, outer, describe);
        var right = RunQuery(set.Right, scope, outer, describe);
        var types = SetTypes(left, right);
        var comparer = new GroupKeyComparer();
        IEnumerable<SqlValue[]> combined = set.Operation switch
        {
            "UNION ALL" => left.Rows.Concat(right.Rows),
            "UNION" => left.Rows.Union(right.Rows, comparer),
            "INTERSECT" => left.Rows.Intersect(right.Rows, comparer),
            "EXCEPT" => left.Rows.Except(right.Rows, comparer),
            "INTERSECT ALL" or "EXCEPT ALL" => Multiset(left.Rows, right.Rows, set.Operation == "INTERSECT ALL", comparer),
            _ => throw new SqlException("Unknown set operator.")
        };
        IOrderedEnumerable<SqlValue[]>? ordered = null;
        var comparison = Comparer<SqlValue>.Create(SqlValue.Compare);
        foreach (var sort in query.Order)
        {
            var candidates = Enumerable.Range(0, left.Columns.Count).Where(i => SqlNameComparer.Equals(left.Columns[i], sort.Column)).ToArray();
            if (sort.Qualifier is not null || candidates.Length != 1)
                throw new SqlException($"Unknown or ambiguous set output '{sort.Column}'.");
            var index = candidates[0];
            ordered = ordered is null ?
                sort.Descending ? combined.OrderByDescending(r => r[index], comparison) : combined.OrderBy(r => r[index], comparison) :
                sort.Descending ? ordered.ThenByDescending(r => r[index], comparison) : ordered.ThenBy(r => r[index], comparison);
        }
        var rows = (ordered ?? combined).Skip(query.Offset).Take(query.Limit ?? MaxRows + 1).ToArray();
        return new(left.Columns, rows, $"{rows.Length} rows") { ColumnTypes = types };
    }

    private static IEnumerable<SqlValue[]> Multiset(IReadOnlyList<SqlValue[]> left, IReadOnlyList<SqlValue[]> right,
        bool intersect, IEqualityComparer<SqlValue[]> comparer)
    {
        var counts = right.GroupBy(row => row, comparer).ToDictionary(g => g.Key, g => g.Count(), comparer);
        foreach (var row in left)
        {
            var matches = counts.GetValueOrDefault(row);
            if (matches != 0) counts[row] = matches - 1;
            if (intersect == (matches != 0)) yield return row;
        }
    }
}
