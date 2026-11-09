using System.Diagnostics;
using System.Globalization;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed record PlanStep(string Operation, string? Relation, string? Detail,
        double? EstimatedRows = null, double? EstimatedCost = null, double? FilteredRows = null);

    private async Task<SqlResult> ExplainAsync(SelectRows query)
    {
        var steps = new List<PlanStep>();
        await ExplainSelectAsync(query, steps);
        return FormatPlan(query, steps);
    }

    private static SqlResult FormatPlan(SelectRows query, IReadOnlyList<PlanStep> steps)
    {
        var rows = WithPipelineEstimates(query, steps).Select(MakePlanRow).ToArray();
        return new(PlanColumns, rows, $"{rows.Length} plan steps") { ColumnTypes = PlanColumnTypes };
    }

    private async Task<SqlResult> ExplainAnalyzeAsync(SelectRows query)
    {
        var profile = new QueryProfile();
        long started = profile.Start();
        var result = await ExecuteStatementAsync(query, profile: profile);
        profile.Record(new("RESULT", null, "SELECT execution, including child operations"), result.Rows.Count, started);
        return FormatProfile(query, profile);
    }

    private static SqlResult FormatProfile(SelectRows query, QueryProfile profile)
    {
        var plans = WithPipelineEstimates(query, profile.Measurements.Select(sample => sample.Step).ToArray());
        var rows = plans.Select((step, i) =>
        {
            var sample = profile.Measurements[i];
            SqlValue average = sample.Loops == 0 ? SqlValue.Null : SqlNumbers.Number((decimal)sample.Rows / sample.Loops);
            SqlValue[] metrics = [SqlNumbers.Number(sample.Rows), SqlNumbers.Number(sample.Loops), average,
                SqlNumbers.Number(decimal.Round(sample.ElapsedTicks * 1000m / Stopwatch.Frequency, 6))];
            return (SqlValue[])[.. MakePlanRow(step, i), .. metrics];
        }).ToArray();
        return new([.. PlanColumns, "actual_rows", "loops", "actual_rows_per_loop", "elapsed_ms"], rows, $"{rows.Length} measured plan steps")
            { ColumnTypes = [.. PlanColumnTypes, "INTEGER", "INTEGER", "REAL", "REAL"] };
    }

    private static IReadOnlyList<string> PlanColumns =>
        ["step", "operation", "relation", "detail", "estimated_rows", "estimated_cost"];
    private static IReadOnlyList<string> PlanColumnTypes => ["INTEGER", "TEXT", "TEXT", "TEXT", "REAL", "REAL"];

    private static IEnumerable<PlanStep> WithPipelineEstimates(SelectRows query, IReadOnlyList<PlanStep> steps)
    {
        // Nested materializations and subqueries have independent input cardinalities.
        // A flat list of their plan steps cannot propagate one estimate across scopes.
        bool singleSource = !query.HasQuerySources && query.Join is null &&
            !steps.Any(step => step.Operation == "MATERIALIZE VIEW");
        double? rows = null;
        double? filteredRows = null;
        foreach (var step in steps)
        {
            if (!singleSource) { yield return step; continue; }
            if (step.FilteredRows is { } estimate) filteredRows = estimate;
            rows = step.EstimatedRows ?? step.Operation switch
            {
                "FILTER" when step.Detail == "WHERE" => filteredRows,
                "SORT" or "WINDOW" or "PROJECT" or "RESULT" => rows,
                "AGGREGATE" when query.GroupBy is null => 1,
                "LIMIT" when query.Limit == 0 => 0,
                "LIMIT" when rows is { } input => Math.Clamp(input - query.Offset, 0, query.Limit ?? MaxRows),
                _ => null
            };
            yield return step with { EstimatedRows = rows };
        }
    }

    private static SqlValue[] MakePlanRow(PlanStep step, int index)
    {
        static SqlValue Text(string? value) => value is null ? SqlValue.Null : SqlValue.String(value);
        static SqlValue Estimate(double? value)
        {
            if (value is null) return SqlValue.Null;
            var text = value.Value.ToString("0.####", CultureInfo.InvariantCulture);
            return decimal.TryParse(text, NumberStyles.Number, CultureInfo.InvariantCulture, out var number)
                ? SqlNumbers.Number(number) : SqlValue.Null;
        }
        return [
            SqlNumbers.Number(index + 1), SqlValue.String(step.Operation), Text(step.Relation), Text(step.Detail),
            Estimate(step.EstimatedRows), Estimate(step.EstimatedCost)
        ];
    }

    private async Task<SqlTable> ReadExplainSourceAsync(string name, bool baseOnly)
    {
        if (baseOnly) return await ReadSourceTableAsync(name);
        var bytes = await GetAsync(TableKey(name)) ?? await GetAsync(ViewKey(name))
            ?? throw new SqlException($"Table or view '{name}' does not exist.");
        return Deserialize<SqlTable>(bytes);
    }

    private async Task ExplainSelectAsync(SelectRows query, List<PlanStep> steps, bool baseOnly = false)
    {
        if (query.HasQuerySources)
        {
            await ExplainRelationsAsync(query, steps);
            return;
        }
        if (query.Join is not null)
        {
            var sources = new List<SqlTable>();
            foreach (var name in query.Joins().Select(j => j.Table).Prepend(query.Table!))
                sources.Add(await ReadExplainSourceAsync(name, baseOnly));
            var join = BindJoin(query, sources);
            Select(join.Query, join.Schema, [], describeOnly: true);
            foreach (var source in sources.DistinctBy(table => table.Name, SqlNameComparer))
                await ExplainMaterializationAsync(source, steps);
            steps.AddRange(ExplainJoinAccess(query, join, sources, ChooseJoinOrder(query, join, sources)));
        }
        else
        {
            var source = query.Table is null ? new SqlTable("", []) : await ReadExplainSourceAsync(query.Table, baseOnly);
            var bound = BindSingleSource(query, source);
            Select(bound, source, [], describeOnly: true);
            if (query.Table is null) steps.Add(new("CONSTANT ROW", null, null, 1, 0));
            else if (source.ViewQuery is null) steps.Add(ExplainTableAccess(bound, source));
            else
            {
                await ExplainSelectAsync(await ReadViewQueryAsync(source), steps, baseOnly: true);
                steps.Add(new("MATERIALIZE VIEW", source.Name, null));
            }
        }
        AppendSelectSteps(query, steps);
    }

    private async Task ExplainRelationsAsync(SelectRows query, List<PlanStep> steps)
    {
        var inputs = new Dictionary<string, Relation>(SqlNameComparer);
        foreach (var name in RequiredSources(query, new(SqlNameComparer)).Distinct(SqlNameComparer))
        {
            var source = await ReadExplainSourceAsync(name, baseOnly: false);
            inputs.Add(name, new(source, []));
            await ExplainMaterializationAsync(source, steps);
        }
        RunQuery(query, inputs, null, describe: true, finalPage: true, explain: steps);
    }

    private async Task ExplainMaterializationAsync(SqlTable source, List<PlanStep> steps)
    {
        if (source.ViewQuery is null)
        {
            var rows = EstimateTableRows(source);
            steps.Add(new("TABLE SCAN", source.Name, "Materialize query input", rows, 1d + rows));
            return;
        }
        await ExplainSelectAsync(await ReadViewQueryAsync(source), steps, baseOnly: true);
        steps.Add(new("MATERIALIZE VIEW", source.Name, null));
    }

    private static void AppendSelectSteps(SelectRows query, List<PlanStep> steps)
    {
        if (query.Where is not null) steps.Add(new("FILTER", null, "WHERE"));
        if (query.IsGrouped) steps.Add(new("AGGREGATE", null, query.GroupDescription ?? "Single group"));
        if (query.Having is not null) steps.Add(new("FILTER", null, "HAVING"));
        foreach (var window in query.Projections.Select(p => p.Window).OfType<SqlWindow>())
            steps.Add(new("WINDOW", null, window.Function));
        AppendSortAndPage(query, steps);
    }

    private static void AppendSortAndPage(SelectRows query, List<PlanStep> steps)
    {
        if (query.Order.Count > 0) steps.Add(new("SORT", null,
            string.Join(", ", query.Order.Select(order => order.Description))));
        if (query.Distinct) steps.Add(new("DISTINCT", null, "Remove duplicate projected rows"));
        if (query.Offset > 0 || query.Limit.HasValue)
            steps.Add(new("LIMIT", null, $"Offset {query.Offset}, limit {query.Limit ?? MaxRows}"));
    }
}
