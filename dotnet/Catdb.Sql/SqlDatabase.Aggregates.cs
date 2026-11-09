using System.Globalization;

namespace Catdb.Sql;

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

    private static SqlResult SelectGroups(SelectRows select, SqlTable table, List<SqlRow> rows)
    {
        var keys = select.GroupBy?.Select(f => f.Name).ToArray() ?? [];
        var groupedColumns = keys.ToHashSet(StringComparer.OrdinalIgnoreCase);
        foreach (var projection in select.Projections)
        {
            if (projection.Expression is not SqlField field) continue;
            if (projection.Aggregate.HasValue)
            {
                if (Column(table, field.Name).Type is not ("INTEGER" or "REAL"))
                    throw new SqlException($"SUM / AVG には数値列を指定してください: '{field.Name}'。");
            }
            else if (!groupedColumns.Contains(field.Name)) throw new SqlException($"列 '{field.Name}' を GROUP BY に指定してください。");
        }
        foreach (var sort in select.Order)
            if (sort.OutputIndex is null && !groupedColumns.Contains(sort.Column))
                throw new SqlException($"ORDER BY の列 '{sort.Column}' を GROUP BY に指定してください。");

        // A global aggregate returns one group even if its input is empty.
        IEnumerable<List<SqlRow>> groups = keys.Length == 0 ? [rows] :
            rows.GroupBy(r => keys.Select(k => r.Values[k]).ToArray(), new GroupKeyComparer()).Select(g => g.ToList());
        var results = new List<(SqlValue[] Values, SqlValue[] SortKeys)>();
        foreach (var group in groups)
        {
            var values = select.Projections.Select(p => AggregateValue(p, group)).ToArray();
            var sortKeys = select.Order.Select(o => o.OutputIndex is int index ? values[index] : group[0].Values[o.Column]).ToArray();
            results.Add((values, sortKeys));
        }
        if (select.Order.Count > 0)
            results.Sort((left, right) =>
            {
                for (int i = 0; i < select.Order.Count; i++)
                {
                    var comparison = SqlValue.Compare(left.SortKeys[i], right.SortKeys[i]);
                    if (comparison != 0) return select.Order[i].Descending ? -comparison : comparison;
                }
                return 0;
            });
        var page = results.Skip(select.Offset).Take(select.Limit ?? MaxRows).Select(g => g.Values).ToArray();
        return new(select.Projections.Select(p => p.Alias ?? "value").ToArray(), page, $"{page.Length} 行を取得しました。");
    }

    private static SqlValue AggregateValue(SqlProjection projection, List<SqlRow> group)
    {
        if (projection.Count) return SqlValue.Number(group.Count.ToString(CultureInfo.InvariantCulture));
        if (projection.Expression is SqlLiteral literal) return literal.Value;
        var field = (SqlField)projection.Expression!;
        if (projection.Aggregate is null) return group[0].Values[field.Name];
        decimal sum = 0;
        int count = 0;
        try
        {
            foreach (var row in group)
            {
                var value = row.Values[field.Name];
                if (value.IsNull) continue;
                sum += decimal.Parse(value.Text!, CultureInfo.InvariantCulture);
                count++;
            }
        }
        catch (OverflowException) { throw new SqlException("集計の合計値が decimal の範囲を超えました。"); }
        if (count == 0) return SqlValue.Null;
        var result = projection.Aggregate == NumericAggregate.Average ? sum / count : sum;
        return SqlValue.Number(result.ToString(CultureInfo.InvariantCulture));
    }
}
