using System.Globalization;

namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    private sealed class GroupKeyComparer : IEqualityComparer<SqlValue[]>
    {
        public bool Equals(SqlValue[]? left, SqlValue[]? right) =>
            left is not null && right is not null && left.Length == right.Length &&
            left.Zip(right).All(pair => SqlValue.Compare(pair.First, pair.Second) == 0);

        public int GetHashCode(SqlValue[] key)
        {
            var hash = new HashCode();
            foreach (var value in key)
            {
                if (value.Kind == "NUMBER") hash.Add(decimal.Parse(value.Text!, CultureInfo.InvariantCulture));
                else hash.Add(value.Text, StringComparer.Ordinal);
            }
            return hash.ToHashCode();
        }
    }

    private static SqlResult SelectGroups(SelectRows select, SqlTable table, List<SqlRow> rows, bool describeOnly, SelectTimings? timings)
    {
        var keys = select.GroupBy ?? [];
        foreach (var key in keys) ExpressionType(key, table.Columns);
        var groupedColumns = keys.OfType<SqlField>().Select(field => field.Name).ToHashSet(StringComparer.OrdinalIgnoreCase);
        void RequireGrouped(string field)
        {
            if (!groupedColumns.Contains(field)) throw new SqlException($"Column '{field}' must be grouped or aggregated.");
        }
        void ValidateGrouped(SqlExpression? expression)
        {
            expression?.Rewrite(node =>
            {
                if (node is SqlAggregate || keys.Any(key => SameGroupingExpression(key, node))) return node;
                if (node is SqlField field) RequireGrouped(field.Name);
                if (node is SqlBoundQuery query)
                    foreach (var column in query.CorrelatedFields) RequireGrouped(column);
                return null;
            });
        }

        var types = ProjectionTypes(select.Projections, table);
        var expressions = select.Projections.Select(p => p.Count ? new SqlAggregate("COUNT", null) :
            p.Aggregate.HasValue ? new SqlAggregate(p.Aggregate == NumericAggregate.Sum ? "SUM" : "AVG", p.Expression) :
            p.Window?.Argument ?? p.Expression).ToArray();
        foreach (var expression in expressions) ValidateGrouped(expression);
        ValidateGrouped(select.Having);
        if (select.Having is { } filter) RequireType(ExpressionType(filter, table.Columns), "BOOLEAN");
        foreach (var window in select.Projections.Select(p => p.Window).OfType<SqlWindow>())
        {
            ValidateGrouped(window.Filter);
            foreach (var field in window.Partition) RequireGrouped(field.Name);
            foreach (var sort in window.Order) RequireGrouped(sort.Column);
        }
        foreach (var sort in select.Order)
            if (sort.OutputIndex is null) RequireGrouped(sort.Column);

        // A global aggregate returns one group even when no input row survives WHERE.
        IEnumerable<List<SqlRow>> groups = describeOnly ? [] : keys.Count == 0 ? [rows] :
            rows.GroupBy(r => keys.Select(k => k.Evaluate(r.Values)).ToArray(), new GroupKeyComparer()).Select(g => g.ToList());
        if (timings?.Groups is { } groupStage) groups = groupStage.Observe(groups);
        var groupRows = new List<SqlRow>();
        var results = new List<SqlValue[]>();
        foreach (var group in groups)
        {
            var grouped = keys.Count == 0 ? new Dictionary<string, SqlValue>(StringComparer.OrdinalIgnoreCase)
                : new Dictionary<string, SqlValue>(group[0].Values, StringComparer.OrdinalIgnoreCase);
            var aggregates = new Dictionary<SqlAggregate, SqlValue>();
            SqlValue Evaluate(SqlExpression expression) => expression.Rewrite(node =>
            {
                if (node is not SqlAggregate aggregate) return null;
                if (!aggregates.TryGetValue(aggregate, out var value))
                {
                    var started = timings?.Groups?.Start() ?? 0;
                    aggregates[aggregate] = value = EvaluateAggregate(aggregate, group);
                    timings?.Groups?.End(started, 0);
                }
                return new SqlLiteral(value);
            }).Evaluate(grouped);

            if (select.Having is not null)
            {
                var started = timings?.Having?.Start() ?? 0;
                bool accepted = Evaluate(select.Having).IsTrue;
                timings?.Having?.End(started, accepted ? 1 : 0);
                if (!accepted) continue;
            }
            var projectionStart = timings?.Project?.Start() ?? 0;
            var values = new SqlValue[expressions.Length];
            for (int i = 0; i < expressions.Length; i++)
            {
                var accepted = true;
                if (select.Projections[i].Window?.Filter is { } windowFilter)
                {
                    var keep = Evaluate(windowFilter);
                    grouped[WindowFilterKey(i)] = keep;
                    accepted = keep.IsTrue;
                }
                values[i] = accepted && expressions[i] is { } expr ? Evaluate(expr) : SqlValue.Null;
                if (select.Projections[i].Window?.Argument is not null)
                    grouped[WindowArgumentKey(i)] = values[i];
            }
            groupRows.Add(new("", grouped));
            results.Add(values);
            timings?.Project?.End(projectionStart, 1);
        }

        // Window functions see the surviving groups before the final ORDER BY and pagination.
        for (int i = 0; i < select.Projections.Count; i++)
        {
            if (select.Projections[i].Window is not { } window) continue;
            if (window.Argument is not null) window = window with { Argument = new SqlField(WindowArgumentKey(i)) };
            if (window.Filter is not null) window = window with { Filter = new SqlField(WindowFilterKey(i)) };
            var values = WindowValues(window, groupRows, timings?.Windows.GetValueOrDefault(i));
            for (int row = 0; row < results.Count; row++) results[row][i] = values[row];
        }
        var order = Enumerable.Range(0, results.Count);
        if (select.Order.Count != 0)
            order = order.Order(Comparer<int>.Create((left, right) =>
            {
                foreach (var sort in select.Order)
                {
                    SqlValue SortValue(int i) => sort.OutputIndex is int index ? results[i][index] : groupRows[i].Values[sort.Column];
                    var comparison = sort.Compare(SortValue(left), SortValue(right));
                    if (comparison != 0) return comparison;
                }
                return 0;
            }));
        if (timings?.Sort is { } sortStage) order = sortStage.Observe(order);
        var page = PageProjectedRows(select, order.Select(i => results[i]), timings);
        return new(select.Projections.Select(p => p.Alias ?? "value").ToArray(), page, $"{page.Length} rows")
            { ColumnTypes = types };
    }

    // SQL identifiers cannot contain control characters, so these keys cannot collide with user columns.
    private static string WindowArgumentKey(int index) => $"\0window{index}";
    private static string WindowFilterKey(int index) => $"\0windowFilter{index}";

    // Record equality alone compares CASE branches and IN lists by collection identity.
    private static bool SameGroupingExpression(SqlExpression? a, SqlExpression? b) => (a, b) switch
    {
        (SqlCase x, SqlCase y) => SameGroupingExpression(x.Operand, y.Operand) && SameGroupingExpression(x.Else, y.Else)
            && x.Branches.Count == y.Branches.Count && x.Branches.Zip(y.Branches).All(p =>
                SameGroupingExpression(p.First.When, p.Second.When) && SameGroupingExpression(p.First.Then, p.Second.Then)),
        (SqlInList x, SqlInList y) => SameGroupingExpression(x.Value, y.Value) && x.Items.Count == y.Items.Count &&
            x.Items.Zip(y.Items).All(p => SameGroupingExpression(p.First, p.Second)),
        (SqlBinary x, SqlBinary y) => x.Operator == y.Operator && SameGroupingExpression(x.Left, y.Left) && SameGroupingExpression(x.Right, y.Right),
        (SqlUnary x, SqlUnary y) => x.Operator == y.Operator && SameGroupingExpression(x.Operand, y.Operand),
        (SqlIsNull x, SqlIsNull y) => x.Negated == y.Negated && SameGroupingExpression(x.Expression, y.Expression),
        (SqlCast x, SqlCast y) => x.TargetType == y.TargetType && x.MaxLength == y.MaxLength && SameGroupingExpression(x.Operand, y.Operand),
        (SqlBetween x, SqlBetween y) => SameGroupingExpression(x.Value, y.Value) && SameGroupingExpression(x.Lower, y.Lower) &&
            SameGroupingExpression(x.Upper, y.Upper),
        _ => Equals(a, b)
    };

    private static SqlValue EvaluateAggregate(SqlAggregate aggregate, IReadOnlyList<SqlRow> group)
    {
        if (aggregate.Argument is null)
            return SqlNumbers.Number(aggregate.Filter is null ? group.Count : group.Count(r => aggregate.Filter.Evaluate(r.Values).IsTrue));
        var seen = aggregate.Distinct ? new HashSet<string>(StringComparer.Ordinal) : null;
        SqlValue extreme = SqlValue.Null;
        decimal sum = 0;
        int count = 0;
        try
        {
            foreach (var row in group)
            {
                if (aggregate.Filter is not null && !aggregate.Filter.Evaluate(row.Values).IsTrue) continue;
                var value = aggregate.Argument.Evaluate(row.Values);
                if (value.IsNull || seen is not null && !seen.Add(UniqueIdentity(value))) continue;
                count++;
                switch (aggregate.Function)
                {
                    case "SUM" or "AVG": sum += SqlNumbers.Numeric(value); break;
                    case "MIN" when extreme.IsNull || SqlValue.Compare(value, extreme) < 0: extreme = value; break;
                    case "MAX" when extreme.IsNull || SqlValue.Compare(value, extreme) > 0: extreme = value; break;
                }
            }
        }
        catch (OverflowException) { throw new SqlException("Aggregate sum exceeds the decimal range."); }
        return aggregate.Function switch
        {
            "COUNT" => SqlNumbers.Number(count),
            "MIN" or "MAX" => extreme,
            _ when count == 0 => SqlValue.Null,
            "SUM" => SqlNumbers.Number(sum),
            "AVG" => SqlNumbers.Number(sum / count),
            _ => throw new SqlException($"Unknown aggregate '{aggregate.Function}'.")
        };
    }
}
