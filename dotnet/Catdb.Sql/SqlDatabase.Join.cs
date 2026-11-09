namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed record BoundColumn(SqlColumn Column, string Qualifier, string Key, int Source);

    private sealed class SelectScope
    {
        private readonly List<BoundColumn> columns = [];
        private readonly HashSet<string> selectedKeys = new(StringComparer.OrdinalIgnoreCase);
        private readonly HashSet<string> qualifiers = new(StringComparer.OrdinalIgnoreCase);
        public BoundColumn[] ResultColumns() => columns.Where(c => selectedKeys.Contains(c.Key)).ToArray();

        public SelectScope(SelectRows query, SqlTable left) =>
            AddSource(left, query.TableAlias ?? left.Name, 0, query.Join is not null);

        public void AddSource(SqlTable table, string qualifier, int source, bool joined)
        {
            if (!qualifiers.Add(qualifier))
                throw new SqlException($"結合元の名前 '{qualifier}' が重複しています。異なる別名を指定してください。");
            var prefix = joined ? $"{source}:" : "";
            foreach (var column in table.Columns)
                columns.Add(new(column, qualifier, prefix + column.Name, source));
        }

        public BoundColumn Resolve(SqlField field)
        {
            var candidates = columns.Where(c => (field.Qualifier is null || Same(c.Qualifier, field.Qualifier)) &&
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
                    var expanded = columns.Where(c => p.Qualifier is null || Same(c.Qualifier, p.Qualifier)).ToList();
                    if (expanded.Count == 0) throw new SqlException("SELECT * の対象となるテーブルが見つかりません。");
                    projections.AddRange(expanded.Select(c => new SqlProjection(new SqlField(SelectKey(c)), c.Column.Name)));
                }
                else projections.Add(p with { Expression = BindExpression(p.Expression) });
            }
            if (projections.Count > 64) throw new SqlException("結果列は64列以下にしてください。");
            var groupBy = query.GroupBy?.Select(f => new SqlField(SelectKey(Resolve(f)))).ToList();
            var order = new List<SqlOrder>();
            foreach (var sort in query.Order)
            {
                if (query.IsGrouped && sort.Qualifier is null)
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
            return query with { Projections = projections, Where = BindExpression(query.Where), Order = order, GroupBy = groupBy };
        }

        private SqlExpression? BindExpression(SqlExpression? expression) => expression switch
        {
            null or SqlLiteral => expression,
            SqlField field => new SqlField(SelectKey(Resolve(field))),
            SqlInValues membership => BindMembership(membership),
            SqlBinary binary => binary with { Left = BindExpression(binary.Left)!, Right = BindExpression(binary.Right)! },
            SqlIsNull test => test with { Expression = BindExpression(test.Expression)! },
            _ => throw new SqlException("未対応の式です。")
        };

        private string SelectKey(BoundColumn column)
        {
            selectedKeys.Add(column.Key);
            return column.Key;
        }

        private SqlInValues BindMembership(SqlInValues membership)
        {
            var column = Resolve(membership.Field);
            var outerType = ComparisonType(column.Column);
            if (outerType != membership.InnerType && outerType != "NULL" && membership.InnerType != "NULL")
                throw new SqlException("IN の両側の列の型が一致しません。");
            return membership with { Field = new SqlField(SelectKey(column)) };
        }

        private static bool Same(string a, string b) => string.Equals(a, b, StringComparison.OrdinalIgnoreCase);
    }

    private static SelectRows BindSingleSource(SelectRows query, SqlTable table) => new SelectScope(query, table).Bind(query);

    private sealed record JoinStep(BoundColumn Left, SqlColumn Right, SqlJoinKind Kind);
    private sealed record BoundJoin(SelectRows Query, SqlTable Schema, BoundColumn[] Columns, List<JoinStep> Steps);

    private static BoundJoin BindJoin(SelectRows query, List<SqlTable> tables)
    {
        var scope = new SelectScope(query, tables[0]);
        var steps = new List<JoinStep>();
        foreach (var join in query.Joins())
        {
            int n = steps.Count + 1;
            scope.AddSource(tables[n], join.Alias ?? tables[n].Name, n, true);
            var first = scope.Resolve(join.Left);
            var second = scope.Resolve(join.Right);
            if ((first.Source == n) == (second.Source == n))
                throw new SqlException("ON は先行する表の列と追加する表の列を等号で結んでください。");
            var (left, right) = first.Source == n ? (second, first) : (first, second);
            var leftType = ComparisonType(left.Column);
            var rightType = ComparisonType(right.Column);
            if (leftType != rightType && leftType != "NULL" && rightType != "NULL")
                throw new SqlException("JOINのONに指定した列の型が一致しません。");
            steps.Add(new(left, right.Column, join.Kind));
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
        var plan = BindJoin(query, sources.Select(s => s.Table).ToList());
        var rows = sources[0].Rows.Select(row => new SqlRow?[] { row }).ToList();
        for (int i = 0; i < plan.Steps.Count; i++)
            rows = JoinRows(rows, sources[i + 1].Rows, plan.Steps[i], i + 1);
        var resultRows = rows.Select(tuple =>
        {
            var values = new Dictionary<string, SqlValue>(plan.Columns.Length, StringComparer.OrdinalIgnoreCase);
            foreach (var col in plan.Columns)
                values[col.Key] = tuple[col.Source]?.Values[col.Column.Name] ?? SqlValue.Null;
            return new SqlRow("", values);
        }).ToList();
        return Select(plan.Query, plan.Schema, resultRows);
    }

    private static List<SqlRow?[]> JoinRows(List<SqlRow?[]> leftRows, List<SqlRow> rightRows, JoinStep step, int source)
    {
        var lookup = rightRows.Where(r => !r.Values[step.Right.Name].IsNull)
            .ToLookup(r => UniqueIdentity(r.Values[step.Right.Name]), StringComparer.Ordinal);
        var pairs = new List<SqlRow?[]>();
        var preserveLeft = step.Kind is SqlJoinKind.Left or SqlJoinKind.Full;
        var matchedRightKeys = step.Kind is SqlJoinKind.Right or SqlJoinKind.Full
            ? new HashSet<string>(StringComparer.Ordinal) : null;
        foreach (var leftRow in leftRows)
        {
            var key = leftRow[step.Left.Source]?.Values[step.Left.Column.Name] ?? SqlValue.Null;
            var identity = key.IsNull ? null : UniqueIdentity(key);
            if (identity is null || !lookup.Contains(identity))
            {
                if (preserveLeft) AppendPair(leftRow, null);
                continue;
            }
            matchedRightKeys?.Add(identity);
            foreach (var rightRow in lookup[identity]) AppendPair(leftRow, rightRow);
        }
        if (matchedRightKeys is not null)
            foreach (var rightRow in rightRows)
            {
                var key = rightRow.Values[step.Right.Name];
                if (key.IsNull || !matchedRightKeys.Contains(UniqueIdentity(key))) AppendPair(null, rightRow);
            }
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

    private static string ComparisonType(SqlColumn column) => column.Type is "INTEGER" or "REAL" ? "NUMBER" : column.Type;
}
