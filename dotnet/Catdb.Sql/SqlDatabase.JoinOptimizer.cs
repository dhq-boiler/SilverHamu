namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed record JoinAccess(int Source, List<(BoundColumn Left, BoundColumn Right)> Keys,
        bool UseHash, double EstimatedRows, double TotalCost);

    private sealed record JoinOrder(int First, List<JoinAccess> Steps, double Rows, double Cost);

    private static JoinOrder? ChooseJoinOrder(SelectRows query, BoundJoin bound, IReadOnlyList<SqlTable> schemas)
    {
        if (schemas.Count > 8 || bound.Steps.Any(step => step.Kind != SqlJoinKind.Inner || step.Filter is not null || step.Keys.Count == 0)
            || query.Joins().Any(join => join.Natural || join.Using is not null))
            return null;

        int count = schemas.Count;
        var edges = bound.Steps.SelectMany(step => step.Keys).ToArray();
        var estimates = schemas.Select(EstimateTableRows).ToArray();
        // Equality is associative here. Retaining the least-cost prefix per subset
        // bounds planning work even for the eight-input case.
        var best = new JoinOrder?[1 << count];
        for (int source = 0; source < count; source++)
            best[1 << source] = new(source, [], estimates[source], estimates[source]);
        for (int mask = 1; mask < best.Length; mask++)
        {
            if (best[mask] is not { } prefix) continue;
            for (int source = 0; source < count; source++)
            {
                int bit = 1 << source;
                if ((mask & bit) != 0) continue;
                var keys = new List<(BoundColumn Left, BoundColumn Right)>();
                foreach (var (a, b) in edges)
                {
                    if (a.Source == source && (mask & (1 << b.Source)) != 0) keys.Add((b, a));
                    if (b.Source == source && (mask & (1 << a.Source)) != 0) keys.Add((a, b));
                }
                if (keys.Count == 0) continue;
                var rightRows = estimates[source];
                double output = EstimateJoinOutput(prefix.Rows, rightRows, keys, schemas);
                double nestedCost = prefix.Rows * rightRows * keys.Count;
                double hashCost = 4 + (prefix.Rows + 2 * rightRows) * keys.Count;
                bool useHash = hashCost < nestedCost;
                double cost = prefix.Cost + rightRows + output + Math.Min(hashCost, nestedCost);
                var next = mask | bit;
                if (best[next] is { } current && current.Cost <= cost) continue;
                best[next] = new(prefix.First, [.. prefix.Steps, new(source, keys, useHash, output, cost)], output, cost);
            }
        }
        return best[^1];
    }

    private static double EstimateJoinOutput(double left, double right,
        List<(BoundColumn Left, BoundColumn Right)> keys, IReadOnlyList<SqlTable> schemas)
    {
        double rows = left * right;
        foreach (var (a, b) in keys)
        {
            int ndvLeft = EstimateDistinctCount(schemas[a.Source], a.Column);
            int ndvRight = EstimateDistinctCount(schemas[b.Source], b.Column);
            rows /= Math.Max(ndvLeft, ndvRight);
        }
        return left == 0 || right == 0 ? 0 : Math.Max(1, rows);
    }

    private static List<SqlRow?[]> ExecuteJoinOrder(JoinOrder? physical, BoundJoin bound,
        List<(SqlTable Table, List<SqlRow> Rows)> sources)
    {
        if (physical is not null)
        {
            int width = sources.Count;
            var rows = sources[physical.First].Rows.Select(row =>
            {
                var tuple = new SqlRow?[width];
                tuple[physical.First] = row;
                return tuple;
            }).ToList();
            bool reordered = IsReordered(physical);
            for (int i = 0; i < physical.Steps.Count; i++)
            {
                var step = physical.Steps[i];
                var joined = ExecuteInnerStep(rows, sources[step.Source].Rows, step);
                if (joined is null)
                {
                    if (reordered && i + 1 < physical.Steps.Count) break;
                    throw new SqlException($"JOINの中間結果が上限{MaxRows}件を超えています。");
                }
                rows = joined;
                if (i + 1 == physical.Steps.Count) return rows;
            }
        }
        // Stale statistics can select an oversized intermediate despite a legal
        // written order. Retry only that order, reusing the same snapshot.
        var written = sources[0].Rows.Select(row => new SqlRow?[] { row }).ToList();
        for (int i = 0; i < bound.Steps.Count; i++)
            written = JoinRows(written, sources[i + 1].Rows, bound.Steps[i], i + 1);
        return written;
    }

    private static List<SqlRow?[]>? ExecuteInnerStep(List<SqlRow?[]> left,
        List<SqlRow> right, JoinAccess step)
    {
        var keys = step.Keys;
        var candidates = right.Select(row => (Row: row, Key: keys.Select(k => row.Values[k.Right.Column.Name]).ToArray()))
            .Where(item => !item.Key.Any(v => v.IsNull)).ToArray();
        var comparer = new GroupKeyComparer();
        var hash = step.UseHash ? candidates.ToLookup(item => item.Key, item => item.Row, comparer) : null;
        var output = new List<SqlRow?[]>();
        foreach (var tuple in left)
        {
            var key = keys.Select(k => k.Left.Read(tuple)).ToArray();
            if (key.Any(v => v.IsNull)) continue;
            var matches = hash is not null ? hash[key] :
                candidates.Where(item => comparer.Equals(key, item.Key)).Select(item => item.Row);
            foreach (var match in matches)
            {
                if (output.Count == MaxRows) return null;
                var joined = (SqlRow?[])tuple.Clone();
                joined[step.Source] = match;
                output.Add(joined);
            }
        }
        return output;
    }

    private static bool IsReordered(JoinOrder order) =>
        order.First != 0 || !order.Steps.Select(step => step.Source).SequenceEqual(Enumerable.Range(1, order.Steps.Count));

    private static IReadOnlyList<PlanStep> ExplainJoinAccess(SelectRows query, BoundJoin bound,
        IReadOnlyList<SqlTable> schemas, JoinOrder? physical)
    {
        var names = query.Joins().Select(join => join.Alias ?? join.Table).Prepend(query.TableAlias ?? query.Table!).ToArray();
        var steps = new List<PlanStep>();
        int first = physical?.First ?? 0;
        var order = physical is null ? "SQL order" : "Cost-based join order";
        if (physical is not null && physical.Steps.Count > 1 && IsReordered(physical))
            order += $"; retry SQL order if an intermediate exceeds {MaxRows} rows";
        steps.Add(new("JOIN INPUT", names[first], order,
            EstimateTableRows(schemas[first]), EstimateTableRows(schemas[first])));
        if (physical is not null)
        {
            foreach (var access in physical.Steps)
                steps.Add(new(access.UseHash ? "HASH JOIN" : "NESTED LOOP JOIN", names[access.Source],
                    "INNER ON " + DescribeJoinKeys(access.Keys), access.EstimatedRows, access.TotalCost));
        }
        else
        {
            for (int i = 0; i < bound.Steps.Count; i++)
            {
                var step = bound.Steps[i];
                var detail = step.Keys.Count == 0 ? "" : " ON " + DescribeJoinKeys(step.Keys);
                if (step.Filter is not null) detail += " with predicate";
                steps.Add(new(step.Keys.Count > 0 ? "HASH JOIN" : "NESTED LOOP JOIN", names[i + 1],
                    step.Kind.ToString().ToUpperInvariant() + detail + " (SQL order)"));
            }
        }
        return steps;
    }

    private static string DescribeJoinKeys(List<(BoundColumn Left, BoundColumn Right)> keys) =>
        string.Join(" AND ", keys.Select(k => $"{k.Left.Qualifier}.{k.Left.Column.Name} = {k.Right.Qualifier}.{k.Right.Column.Name}"));
}
