namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed record BoundColumn(SqlColumn Column, string Qualifier, string Key, int Source,
        IReadOnlyList<BoundColumn>? Inputs = null)
    {
        public SqlValue Read(SqlRow?[] tuple)
        {
            if (Inputs is null) return tuple[Source]?.Values[Column.Name] ?? SqlValue.Null;
            foreach (var input in Inputs)
                if (input.Read(tuple) is { IsNull: false } value) return value;
            return SqlValue.Null;
        }
    }

    private sealed partial class SelectScope
    {
        private readonly List<BoundColumn> columns = [];
        private readonly List<BoundColumn> visible = [];
        private readonly List<BoundColumn> merged = [];
        private readonly HashSet<string> selectedKeys = new(StringComparer.OrdinalIgnoreCase);
        private readonly HashSet<string> qualifiers = new(StringComparer.OrdinalIgnoreCase);
        public BoundColumn[] ResultColumns() => columns.Concat(merged).Where(c => selectedKeys.Contains(c.Key)).ToArray();

        public IEnumerable<string> VisibleNames => visible.Select(c => c.Column.Name);

        private readonly QueryBinding? queryBinding;

        public SelectScope(SelectRows query, SqlTable left)
        {
            queryBinding = query.Binding;
            AddSource(left, query.TableAlias ?? left.Name, 0, query.Join is not null);
        }

        public void AddSource(SqlTable table, string qualifier, int source, bool joined)
        {
            if (!qualifiers.Add(qualifier))
                throw new SqlException($"結合元の名前 '{qualifier}' が重複しています。異なる別名を指定してください。");
            var prefix = joined ? $"{source}:" : "";
            foreach (var column in table.Columns)
            {
                var bound = new BoundColumn(column, qualifier, prefix + column.Name, source);
                columns.Add(bound);
                visible.Add(bound);
            }
        }

        public void MergeKeys(List<(BoundColumn Left, BoundColumn Right)> pairs)
        {
            var leading = new List<BoundColumn>();
            foreach (var (left, right) in pairs)
            {
                visible.Remove(left);
                visible.Remove(right);
                var type = CommonType(left.Column.Type, right.Column.Type);
                var column = new BoundColumn(left.Column with { Type = type }, "", $"using:{merged.Count}", -1, [left, right]);
                merged.Add(column);
                leading.Add(column);
            }
            visible.InsertRange(0, leading);
        }

        public BoundColumn Resolve(SqlField field)
        {
            var candidates = (field.Qualifier is null ? visible : columns).Where(c => (field.Qualifier is null || Same(c.Qualifier, field.Qualifier)) &&
                                                Same(c.Column.Name, field.Name)).Take(2).ToArray();
            if (candidates.Length == 1) return candidates[0];
            var name = field.Qualifier is null ? field.Name : $"{field.Qualifier}.{field.Name}";
            throw new SqlException(candidates.Length == 0 ? $"列 '{name}' は存在しません。" : $"列 '{name}' は曖昧です。テーブル名または別名で修飾してください。");
        }

        public SelectRows Bind(SelectRows query)
        {
            var projections = new List<SqlProjection>();
            foreach (var p in query.Projections)
            {
                if (p.Star)
                {
                    var expanded = (p.Qualifier is null ? visible : columns).Where(c => p.Qualifier is null || Same(c.Qualifier, p.Qualifier)).ToList();
                    if (expanded.Count == 0) throw new SqlException("SELECT * の対象となるテーブルが見つかりません。");
                    projections.AddRange(expanded.Select(c => new SqlProjection(new SqlField(SelectKey(c)), c.Column.Name)));
                }
                else projections.Add(p with { Expression = BindExpression(p.Expression), Window = BindWindow(p.Window) });
            }
            if (projections.Count > 64) throw new SqlException("結果列は64列以下にしてください。");
            var groupBy = query.GroupBy?.Select(f => new SqlField(SelectKey(Resolve(f)))).ToList();
            var order = new List<SqlOrder>();
            foreach (var sort in query.Order)
            {
                if (sort.Qualifier is null && (query.IsGrouped || query.Projections.Any(p =>
                        Same(p.Alias ?? "", sort.Column) && (p.Expression is not SqlField f || !Same(f.Name, sort.Column)))))
                {
                    int first = projections.FindIndex(p => string.Equals(p.Alias, sort.Column, StringComparison.OrdinalIgnoreCase));
                    if (first >= 0)
                    {
                        if (projections.FindIndex(first + 1, p => string.Equals(p.Alias, sort.Column, StringComparison.OrdinalIgnoreCase)) >= 0)
                            throw new SqlException($"ORDER BY の出力列 '{sort.Column}' が曖昧です。");
                        order.Add(sort with { OutputIndex = first });
                        continue;
                    }
                }
                order.Add(new(SelectKey(Resolve(new(sort.Column, sort.Qualifier))), sort.Descending));
            }
            return query with { Projections = projections, Where = BindExpression(query.Where),
                Having = BindExpression(query.Having), Order = order, GroupBy = groupBy };
        }

        private SqlExpression? BindExpression(SqlExpression? expression) => expression switch
        {
            null or SqlLiteral => expression,
            SqlField field => BindField(field),
            SqlNestedQuery nested => BindNestedQuery(nested),
            _ => expression.MapChildren(child => BindExpression(child)!)
        };

        private string SelectKey(BoundColumn column)
        {
            selectedKeys.Add(column.Key);
            return column.Key;
        }

        private static bool Same(string a, string b) => string.Equals(a, b, StringComparison.OrdinalIgnoreCase);
    }

    private static SelectRows BindSingleSource(SelectRows query, SqlTable table) => new SelectScope(query, table).Bind(query);

    private sealed record JoinStep(List<(BoundColumn Left, BoundColumn Right)> Keys, SqlJoinKind Kind, JoinFilter? Filter = null);
    private sealed record JoinFilter(SqlExpression Predicate, BoundColumn[] Inputs)
    {
        public bool Accepts(SqlRow?[] left, SqlRow right, int source, Dictionary<string, SqlValue> values)
        {
            foreach (var input in Inputs)
                values[input.Key] = input.Source == source ? right.Values[input.Column.Name] : input.Read(left);
            return Matches(Predicate, values);
        }
    }
    private sealed record BoundJoin(SelectRows Query, SqlTable Schema, BoundColumn[] Columns, List<JoinStep> Steps);

    private static IEnumerable<SqlExpression> JoinTerms(SqlExpression predicate)
    {
        if (predicate is SqlBinary { Operator: "AND" } conjunction)
        {
            foreach (var term in JoinTerms(conjunction.Left)) yield return term;
            foreach (var term in JoinTerms(conjunction.Right)) yield return term;
        }
        else yield return predicate;
    }

    private static JoinFilter? BindOn(SqlJoin join, SelectScope scope, int source,
        List<(BoundColumn Left, BoundColumn Right)> keys)
    {
        var predicate = join.On ?? (join.Left is not null && join.Right is not null
            ? new SqlBinary(join.Left, "=", join.Right) : throw new SqlException("Expected a JOIN ON condition."));
        var inputs = new Dictionary<string, BoundColumn>(StringComparer.OrdinalIgnoreCase);
        SqlExpression? filter = null;
        foreach (var term in JoinTerms(predicate))
        {
            if (term is SqlBinary { Operator: "=", Left: SqlField { Qualifier: not null } a, Right: SqlField { Qualifier: not null } b })
            {
                var first = scope.Resolve(a);
                var second = scope.Resolve(b);
                if ((first.Source == source) != (second.Source == source))
                {
                    keys.Add(first.Source == source ? (second, first) : (first, second));
                    continue;
                }
            }
            var bound = term.Rewrite(node =>
            {
                if (node is SqlNestedQuery or SqlAggregate)
                    throw new SqlException("JOIN ON does not support aggregates or subqueries.");
                if (node is not SqlField field) return null;
                var column = scope.Resolve(field);
                inputs[column.Key] = column;
                return new SqlField(column.Key);
            });
            filter = filter is null ? bound : new SqlBinary(filter, "AND", bound);
        }
        if (keys.Count == 0)
            throw new SqlException("JOIN ON requires equality between qualified columns of a preceding input and the new input.");
        if (filter is null) return null;
        var columns = inputs.Values.ToArray();
        ValidatePredicate(filter, columns.Select(c => c.Column with { Name = c.Key }).ToArray());
        return new(filter, columns);
    }

    private static BoundJoin BindJoin(SelectRows query, List<SqlTable> tables)
    {
        var scope = new SelectScope(query, tables[0]);
        var steps = new List<JoinStep>();
        foreach (var join in query.Joins())
        {
            int n = steps.Count + 1;
            var qualifier = join.Alias ?? tables[n].Name;
            var names = join.Natural ? scope.VisibleNames.Intersect(tables[n].Columns.Select(c => c.Name),
                StringComparer.OrdinalIgnoreCase).ToList() : join.Using;
            var leftKeys = names?.Select(name => scope.Resolve(new(name))).ToList();
            scope.AddSource(tables[n], qualifier, n, true);
            var keys = new List<(BoundColumn Left, BoundColumn Right)>();
            JoinFilter? filter = null;
            if (leftKeys is not null)
            {
                foreach (var left in leftKeys)
                    keys.Add((left, scope.Resolve(new(left.Column.Name, qualifier))));
                scope.MergeKeys(keys);
            }
            else if (join.Kind != SqlJoinKind.Cross)
                filter = BindOn(join, scope, n, keys);
            foreach (var (left, right) in keys)
                if (ComparisonType(left.Column) != ComparisonType(right.Column) &&
                    left.Column.Type != "NULL" && right.Column.Type != "NULL")
                    throw new SqlException("JOIN key types must match.");
            steps.Add(new(keys, join.Kind, filter));
        }
        var bound = scope.Bind(query);
        var columns = scope.ResultColumns();
        return new(bound, new("", columns.Select(c => c.Column with { Name = c.Key }).ToList()), columns, steps);
    }

    private Task<SqlResult> SelectJoinAsync(SelectRows query, bool baseTablesOnly = false) =>
        SelectJoinAsync(query, name => LoadJoinSourceAsync(name, baseTablesOnly));

    private static async Task<SqlResult> SelectJoinAsync(SelectRows query, Func<string, Task<(SqlTable Table, List<SqlRow> Rows)>> load)
    {
        var cache = new Dictionary<string, (SqlTable Table, List<SqlRow> Rows)>(StringComparer.OrdinalIgnoreCase);
        var sources = new List<(SqlTable Table, List<SqlRow> Rows)>();
        foreach (var name in query.Joins().Select(j => j.Table).Prepend(query.Table!))
        {
            if (!cache.TryGetValue(name, out var source))
            {
                source = await load(name);
                cache.Add(name, source);
            }
            sources.Add(source);
        }
        return SelectJoined(query, sources);
    }

    private static SqlResult SelectJoined(SelectRows query, List<(SqlTable Table, List<SqlRow> Rows)> sources, bool describeOnly = false,
        List<PlanStep>? explain = null)
    {
        var schemas = sources.Select(s => s.Table).ToList();
        var plan = BindJoin(query, schemas);
        var physical = ChooseJoinOrder(query, plan, schemas);
        if (explain is not null) explain.AddRange(ExplainJoinAccess(query, plan, schemas, physical));
        var rows = ExecuteJoinOrder(physical, plan, sources);
        var resultRows = rows.Select(tuple =>
        {
            var values = new Dictionary<string, SqlValue>(plan.Columns.Length, StringComparer.OrdinalIgnoreCase);
            foreach (var col in plan.Columns)
                values[col.Key] = col.Read(tuple);
            return new SqlRow("", values);
        }).ToList();
        return Select(plan.Query, plan.Schema, resultRows, describeOnly);
    }

    private static List<SqlRow?[]> JoinRows(List<SqlRow?[]> leftRows, List<SqlRow> rightRows, JoinStep step, int source)
    {
        var rightKeys = rightRows.Select((row, index) => (Row: row, Index: index,
            Key: step.Keys.Select(k => row.Values[k.Right.Column.Name]).ToArray())).ToList();
        var comparer = new GroupKeyComparer();
        var lookup = step.Keys.Count == 0 ? null : rightKeys.Where(r => !r.Key.Any(v => v.IsNull)).ToLookup(r => r.Key, comparer);
        var pairs = new List<SqlRow?[]>();
        var preserveLeft = step.Kind is SqlJoinKind.Left or SqlJoinKind.Full;
        var matchedRight = step.Kind is SqlJoinKind.Right or SqlJoinKind.Full ? new bool[rightRows.Count] : null;
        var values = new Dictionary<string, SqlValue>(StringComparer.OrdinalIgnoreCase);
        foreach (var leftRow in leftRows)
        {
            var key = step.Keys.Select(k => k.Left.Read(leftRow)).ToArray();
            bool matched = false;
            if (!key.Any(v => v.IsNull))
            {
                foreach (var candidate in lookup is null ? rightKeys : lookup[key])
                {
                    if (step.Filter?.Accepts(leftRow, candidate.Row, source, values) == false) continue;
                    AppendPair(leftRow, candidate.Row);
                    matched = true;
                    if (matchedRight is not null) matchedRight[candidate.Index] = true;
                }
            }
            if (!matched && preserveLeft) AppendPair(leftRow, null);
        }
        if (matchedRight is not null)
            for (int i = 0; i < rightRows.Count; i++)
                if (!matchedRight[i]) AppendPair(null, rightRows[i]);
        return pairs;

        void AppendPair(SqlRow?[]? leftRow, SqlRow? rightRow)
        {
            if (pairs.Count == MaxRows) throw new SqlException($"JOINの中間結果が上限{MaxRows}件を超えています。");
            var tuple = new SqlRow?[source + 1];
            leftRow?.CopyTo(tuple, 0);
            tuple[source] = rightRow;
            pairs.Add(tuple);
        }
    }

    private static string ComparisonType(SqlColumn column) => IsNumericType(column.Type) ? "NUMBER" : column.Type;
}
