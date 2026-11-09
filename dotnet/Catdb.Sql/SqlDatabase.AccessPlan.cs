namespace Catdb.Sql;

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
                .ToDictionary(f => Digest(f.Value), f => f.Count);
            return new SqlColumnStatistics(frequencies.Sum(f => f.Count), frequencies.Length, frequent);
        });
        var updated = table with { Statistics = new(rows.Count, columns) };
        changes.Add((TableKey(table.Name), Serialize(updated)));
        return Empty($"ANALYZE {table.Name}: {rows.Count} rows");
    }

    private static int EstimateTableRows(SqlTable table) => table.Statistics?.RowCount ?? 256;

    private static int EstimateDistinctCount(SqlTable table, SqlColumn column)
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
        if (column.FrequentValues.TryGetValue(Digest(UniqueIdentity(literal)), out var count))
            return (double)count / analyzed.RowCount;
        var remaining = column.NonNullRows - column.FrequentValues.Values.Sum();
        var distinct = column.DistinctCount - column.FrequentValues.Count;
        return (double)remaining / Math.Max(1, distinct) / analyzed.RowCount;
    }

    private sealed record TableAccess(PlanStep Step, IndexCondition? PrimaryKey = null, IndexRange? Range = null);

    private static PlanStep ExplainTableAccess(SelectRows bound, SqlTable table) => PlanTableAccess(bound, table).Step;

    private static TableAccess PlanTableAccess(SelectRows query, SqlTable table)
    {
        var rows = EstimateTableRows(table);
        double bestCost = 1 + rows;
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

        foreach (var range in CandidateIndexRanges(table, conditions, rows))
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
