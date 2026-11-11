namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    private async Task<SqlResult> AnalyzeTableAsync(string name, List<(string Key, byte[]? Value)> changes)
    {
        var table = await ReadSourceTableAsync(name);
        var rows = await LoadRowsAsync(table);
        var columns = table.Columns.ToDictionary(column => column.Name, column =>
        {
            var frequencies = rows.Select(row => row.Values[column.Name]).Where(value => !value.IsNull)
                .GroupBy(UniqueIdentity, StringComparer.Ordinal)
                .Select(group => (Value: group.Key, Count: group.Count())).ToArray();
            var frequent = frequencies.Where(f => f.Count > 1).OrderByDescending(f => f.Count)
                .ThenBy(f => f.Value, StringComparer.Ordinal).Take(8)
                .ToDictionary(f => Digest(f.Value), f => (long)f.Count);
            return new SqlColumnStatistics(frequencies.Sum(f => f.Count), frequencies.Length, frequent);
        });
        var updated = table with { Statistics = new(rows.Count, columns) };
        changes.Add((TableKey(table.Name), Serialize(updated)));
        return Empty($"ANALYZE {table.Name}: {rows.Count} rows");
    }

    private static long EstimateTableRows(SqlTable table) => table.Statistics?.RowCount ?? 256;

    private static long EstimateDistinctCount(SqlTable table, SqlColumn column)
    {
        if (table.Statistics?.Columns.TryGetValue(column.Name, out var statistics) == true)
            return Math.Max(1, statistics.DistinctCount);
        if (column.Unique) return Math.Max(1, EstimateTableRows(table));
        return (column.DistinctType?.BaseType ?? column.Type) == "BOOLEAN" ? 2 : 16;
    }

    private static double EstimateEqualityFraction(SqlTable table, string name, SqlValue literal)
    {
        if (table.Statistics is not { RowCount: > 0 } analyzed ||
            !analyzed.Columns.TryGetValue(name, out var column))
            return 1.0 / EstimateDistinctCount(table, Column(table, name));
        if (column.FrequentValues.TryGetValue(StatisticsDigest(literal), out var count))
            return (double)count / analyzed.RowCount;
        var remaining = column.NonNullRows - column.FrequentValues.Values.Sum();
        var distinct = column.DistinctCount - column.FrequentValues.Count;
        return (double)remaining / Math.Max(1, distinct) / analyzed.RowCount;
    }

    private sealed record TableAccess(PlanStep Step, IndexCondition? PrimaryKey = null, IndexRange? Range = null);

    private static PlanStep ExplainTableAccess(SelectRows bound, SqlTable table) =>
        DescribeAccess(PlanTableAccess(bound, table), bound, table);

    private static PlanStep DescribeAccess(TableAccess access, SelectRows query, SqlTable table)
    {
        if (query.Where is SqlIsNull { Expression: SqlField field } test && table.Statistics is { } stats &&
            stats.Columns.TryGetValue(Column(table, field.Name).Name, out var column))
        {
            var matching = test.Negated ? column.NonNullRows : stats.RowCount - column.NonNullRows;
            return access.Step with { FilteredRows = Math.Min(access.Step.EstimatedRows ?? matching, matching) };
        }
        // Evaluating user expressions to obtain a selectivity estimate could change query behavior.
        if (query.Where is not (SqlBinary { Left: SqlField, Right: SqlLiteral } or
            SqlBinary { Left: SqlLiteral, Right: SqlField }) ||
            LiteralComparison(query.Where, table) is not { } term) return access.Step;
        var fraction = term.Operator == "="
            ? EstimateEqualityFraction(table, term.Column, term.Value) : 1d / 3;
        var input = EstimateTableRows(table);
        var rows = input == 0 ? 0 : Math.Max(1, input * fraction);
        return access.Step with { FilteredRows = Math.Min(access.Step.EstimatedRows ?? rows, rows) };
    }

    private static TableAccess PlanTableAccess(SelectRows query, SqlTable table)
    {
        var rows = EstimateTableRows(table);
        double bestCost = 1d + rows;
        var stats = table.Statistics is null ? "default estimate" : "ANALYZE statistics";
        var best = new TableAccess(new("TABLE SCAN", table.Name, stats, rows, bestCost));
        var conditions = new List<IndexCondition>();
        CollectIndexConditions(query.Where, table, conditions);
        if (conditions.Count == 0) return best;

        if (table.PrimaryIndex && conditions.Find(term => term.Operator == "=" && Column(table, term.Column).PrimaryKey) is { } equality)
        {
            const double pointCost = 2;
            if (pointCost <= bestCost)
            {
                bestCost = pointCost;
                best = new(new("PRIMARY KEY LOOKUP", table.Name, equality.Column, 1, pointCost), PrimaryKey: equality);
            }
        }

        foreach (var range in CandidateIndexRanges(table, conditions, rows, query.Where))
        {
            // Each candidate requires an index entry and a separate row fetch.
            double cost = 2 + 2 * range.EstimatedRows;
            if (cost >= bestCost) continue;
            bestCost = cost;
            best = new(new("INDEX RANGE SCAN", table.Name, range.Layout.Name, range.EstimatedRows, cost), Range: range);
        }
        return best;
    }
}
