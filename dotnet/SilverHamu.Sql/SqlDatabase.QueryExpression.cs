namespace SilverHamu.Sql;

public sealed partial record SelectRows
{
    internal SqlDatabase.QueryBinding? Binding { get; init; }
}

public sealed partial class SqlDatabase
{
    internal delegate SqlResult RunNestedQuery(SqlNestedQuery site, SelectRows query, Func<SqlField, SqlExpression?> resolveOuter,
        bool describeOnly, bool explain, bool correlated = false);
    internal sealed record QueryBinding(
        RunNestedQuery Run,
        Func<SqlField, SqlExpression?>? Outer,
        Action<SqlNestedQuery, bool>? DescribeSubquery);

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

            var execution = nested.Query;
            if (nested.Kind == SubqueryKind.Exists && execution.SetExpression is null && !execution.IsGrouped &&
                (!execution.Distinct || execution.Offset == 0))
                execution = execution with
                {
                    Projections = [new(new SqlLiteral(SqlValue.Boolean(true)), "exists")],
                    Distinct = false, Order = [], Limit = Math.Min(execution.Limit ?? 1, 1)
                };
            bool samePlan = ReferenceEquals(execution, nested.Query);
            var shape = queryBinding.Run(nested, nested.Query, field => Capture(field, null), describeOnly: true, explain: samePlan);
            if (!samePlan && queryBinding.DescribeSubquery is not null)
                queryBinding.Run(nested, execution, field => Capture(field, null), describeOnly: true, explain: true);
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
            SqlResult? cached = null;
            HashSet<string>? members = null;
            bool hasNull = false;
            bool correlated = capture.Count > 0;
            queryBinding.DescribeSubquery?.Invoke(nested, correlated);
            var dependencies = new HashSet<string>(capture, StringComparer.OrdinalIgnoreCase);
            operand?.Rewrite(node =>
            {
                if (node is SqlField f) dependencies.Add(f.Name);
                if (node is SqlBoundQuery sub) dependencies.UnionWith(sub.CorrelatedFields);
                return null;
            });
            SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
            {
                var result = cached ?? queryBinding.Run(nested, execution, field => Capture(field, row),
                    describeOnly: false, explain: false, correlated: correlated);
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
        Func<string, Task<(SqlTable Table, List<SqlRow> Rows)>> load, QueryProfile? profile = null)
    {
        var tables = new Dictionary<string, Relation>(SqlNameComparer);
        foreach (var name in RequiredSources(query, new(SqlNameComparer)).Distinct(SqlNameComparer))
        {
            var source = await load(name);
            tables.Add(name, new(source.Table, source.Rows));
        }
        return RunQuery(query, tables, null, describe: profile?.IsPlanning == true, finalPage: true, profile: profile);
    }

    private static Relation MakeRelation(string name, SqlResult result, IReadOnlyList<string>? aliases = null)
    {
        var schema = TableFromResult(name, result, aliases);
        return new(schema, RowsFromResult(schema, result));
    }

    private static SqlResult RunQuery(SelectRows query, Dictionary<string, Relation> inherited,
        Func<SqlField, SqlExpression?>? outer, bool describe, bool finalPage = false, List<PlanStep>? explain = null,
        QueryProfile? profile = null)
    {
        // Only the client-facing result has an implicit page size. For a relation consumed by
        // another clause, probe one extra row so a bounded query never silently changes its data.
        bool zeroLimit = !describe && query.Limit == 0;
        long started = zeroLimit ? profile?.Start() ?? 0 : 0;
        describe |= zeroLimit;
        var execution = query with { Limit = describe ? 0 : query.Limit ?? (finalPage ? MaxRows : MaxRows + 1),
            ProfileSource = profile is null ? null : query.ProfileSource ?? query };
        var result = EvaluateQuery(execution, inherited, outer, describe, explain, describe ? profile?.Planning() : profile);
        if (zeroLimit) profile?.Record(new("LIMIT", null, $"Offset {query.Offset}, limit 0"), 0, started);
        if (explain is not null)
        {
            if (query.SetExpression is null) AppendSelectSteps(query, explain);
            else AppendSortAndPage(query, explain);
        }
        if (result.Rows.Count > MaxRows)
            throw new SqlException($"An intermediate query result exceeds {MaxRows} rows.");
        return result;
    }

    private static SqlResult EvaluateQuery(SelectRows query, Dictionary<string, Relation> inherited,
        Func<SqlField, SqlExpression?>? outer, bool describe, List<PlanStep>? explain, QueryProfile? profile)
    {
        var scope = inherited;
        if (query.CommonTables.Count > 0 || query.DerivedSources?.Count > 0)
            scope = new(inherited, SqlNameComparer);
        foreach (var cte in query.CommonTables)
        {
            var selfReference = query.Recursive && ReadsTable(cte.Query, cte.Name);
            var cteProfile = profile?.Child(cte);
            long started = cteProfile?.Start() ?? 0;
            var result = selfReference ? RecursiveCte(cte, scope, outer, describe, explain, cteProfile) :
                RunQuery(cte.Query, scope, outer, describe, explain: explain, profile: cteProfile);
            scope[cte.Name] = MakeRelation(cte.Name, result, cte.Columns);
            explain?.Add(new(selfReference ? "RECURSIVE CTE" : "MATERIALIZE CTE", cte.Name, null));
            cteProfile?.Record(new(selfReference ? "RECURSIVE CTE" : "MATERIALIZE CTE", cte.Name, null), result.Rows.Count, started);
        }
        if (query.SetExpression is { } set)
            return SetResult(query, set, scope, outer, describe, explain, profile);
        if (query.DerivedSources is not null)
            foreach (var (key, derived) in query.DerivedSources)
            {
                var derivedProfile = profile?.Child(derived);
                long started = derivedProfile?.Start() ?? 0;
                scope[key] = MakeRelation(key, RunQuery(derived.Query, scope, outer, describe, explain: explain,
                    profile: derivedProfile), derived.Columns);
                if (explain is not null || profile is not null)
                {
                    var alias = key == query.Table ? query.TableAlias : query.Joins().First(j => j.Table == key).Alias;
                    var step = new PlanStep("MATERIALIZE DERIVED TABLE", alias, null);
                    explain?.Add(step);
                    derivedProfile?.Record(step, scope[key].Rows.Count, started);
                }
            }
        query = query with
        {
            Binding = new((site, nested, frame, shapeOnly, showPlan, correlated) =>
            {
                var child = shapeOnly && !showPlan ? null : profile?.Child(site.Query);
                long started = child?.Start() ?? 0;
                var result = RunQuery(nested, scope, frame, shapeOnly, explain: showPlan ? explain : null, profile: child);
                if (!shapeOnly) child?.Record(SubqueryStep(site.Kind, correlated), result.Rows.Count, started);
                return result;
            }, outer, explain is null && profile is null ? null : (site, correlated) =>
            {
                var step = SubqueryStep(site.Kind, correlated);
                explain?.Add(step);
                profile?.Child(site.Query).Declare(step);
            })
        };
        if (query.Table is null)
        {
            explain?.Add(new("CONSTANT ROW", null, null, 1, 0));
            long started = profile?.Start() ?? 0;
            var unit = new SqlTable("", []);
            List<SqlRow> rows = describe ? [] : [new("", new(SqlNameComparer))];
            profile?.Record(new("CONSTANT ROW", null, null, 1, 0), rows.Count, started);
            return Select(BindSingleSource(query, unit), unit, rows, describe, profile: profile);
        }
        if (query.Join is not null)
        {
            var sources = new List<(SqlTable Table, List<SqlRow> Rows)>();
            foreach (var name in query.Joins().Select(j => j.Table).Prepend(query.Table))
            {
                var source = scope[name];
                sources.Add((source.Table, describe ? [] : source.Rows));
            }
            return SelectJoined(query, sources, describe, explain, profile);
        }
        long read = profile?.Start() ?? 0;
        var table = scope[query.Table];
        explain?.Add(new("READ MATERIALIZED INPUT", query.TableAlias ?? query.Table, null));
        profile?.Record(new("READ MATERIALIZED INPUT", query.TableAlias ?? query.Table, null), table.Rows.Count, read);
        return Select(BindSingleSource(query, table.Table), table.Table, describe ? [] : table.Rows, describe, profile: profile);
    }

    private static PlanStep SubqueryStep(SubqueryKind kind, bool correlated) =>
        new("SUBQUERY", null, $"{kind}, {(correlated ? "per outer row" : "cached per query invocation")}");

    private static bool ReadsTable(SelectRows query, string name) =>
        RequiredSources(query, new(SqlNameComparer)).Contains(name, SqlNameComparer);

    private static SqlResult RecursiveCte(SqlCommonTable cte, Dictionary<string, Relation> scope,
        Func<SqlField, SqlExpression?>? outer, bool describe, List<PlanStep>? explain, QueryProfile? profile)
    {
        var body = cte.Query;
        if (body.SetExpression is not { Operation: "UNION" or "UNION ALL" } union ||
            ReadsTable(union.Left, cte.Name) || !ReadsTable(union.Right, cte.Name))
            throw new SqlException("A recursive CTE requires a nonrecursive anchor UNION [ALL] a recursive term.");
        if (body.CommonTables.Count != 0 || body.Order.Count != 0 || body.Limit.HasValue || body.Offset != 0)
            throw new SqlException("Place ordering and pagination in the query reading the recursive CTE.");
        long started = profile?.Start() ?? 0;
        var anchor = RunQuery(union.Left, scope, outer, describe, explain: explain, profile: profile?.Child(union.Left));
        var seen = union.Operation == "UNION" ? new HashSet<SqlValue[]>(new GroupKeyComparer()) : null;
        var rows = anchor.Rows.Where(row => seen?.Add(row) != false).ToList();
        var delta = (IReadOnlyList<SqlValue[]>)rows.ToArray();
        var local = new Dictionary<string, Relation>(scope, SqlNameComparer);
        var types = anchor.ColumnTypes;
        do
        {
            local[cte.Name] = MakeRelation(cte.Name, anchor with { Rows = delta, ColumnTypes = types }, cte.Columns);
            var step = RunQuery(union.Right, local, outer, describe || delta.Count == 0, explain: explain,
                profile: profile?.Child(union.Right));
            types = SetTypes(anchor with { ColumnTypes = types }, step);
            delta = step.Rows.Where(row => seen?.Add(row) != false).ToArray();
            if (rows.Count + delta.Count > MaxRows)
                throw new SqlException($"A recursive CTE may produce at most {MaxRows} rows.");
            rows.AddRange(delta);
        } while (!describe && delta.Count > 0);
        explain?.Add(new(union.Operation, cte.Name, "Combine anchor and recursive term until no new rows"));
        profile?.Record(new(union.Operation, cte.Name, "Combine anchor and recursive term until no new rows"), rows.Count, started);
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
        Func<SqlField, SqlExpression?>? outer, bool describe, List<PlanStep>? explain, QueryProfile? profile)
    {
        var left = RunQuery(set.Left, scope, outer, describe, explain: explain, profile: profile?.Child(set.Left));
        var right = RunQuery(set.Right, scope, outer, describe, explain: explain, profile: profile?.Child(set.Right));
        explain?.Add(new(set.Operation, null, "Combine query results"));
        using var timings = profile is null ? null : new SelectTimings(profile, query);
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
        if (timings?.Combine is { } combine) combined = combine.Observe(combined);
        IOrderedEnumerable<SqlValue[]>? ordered = null;
        foreach (var sort in query.Order)
        {
            var comparison = Comparer<SqlValue>.Create(sort.Compare);
            var candidates = Enumerable.Range(0, left.Columns.Count).Where(i => SqlNameComparer.Equals(left.Columns[i], sort.Column)).ToArray();
            if (sort.Qualifier is not null || candidates.Length != 1)
                throw new SqlException($"Unknown or ambiguous set output '{sort.Column}'.");
            var index = candidates[0];
            ordered = ordered is null ? combined.OrderBy(r => r[index], comparison) : ordered.ThenBy(r => r[index], comparison);
        }
        var sorted = ordered is not null && timings?.Sort is { } sortStage ? sortStage.Observe(ordered) : ordered ?? combined;
        var page = sorted.Skip(query.Offset).Take(query.Limit ?? MaxRows + 1);
        if (timings?.Page is { } limit) page = limit.Observe(page);
        var rows = page.ToArray();
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
