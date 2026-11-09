using System.Globalization;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed record PlanStep(string Operation, string? Relation, string? Detail,
        double? EstimatedRows = null, double? EstimatedCost = null);

    private async Task<SqlResult> ExplainAsync(SelectRows query)
    {
        var steps = new List<PlanStep>();
        await ExplainSelectAsync(query, steps);
        SqlValue Text(string? value) => value is null ? SqlValue.Null : SqlValue.String(value);
        SqlValue Estimate(double? value)
        {
            if (value is null) return SqlValue.Null;
            var text = value.Value.ToString("0.####", CultureInfo.InvariantCulture);
            return decimal.TryParse(text, NumberStyles.Number, CultureInfo.InvariantCulture, out var number)
                ? SqlNumbers.Number(number) : SqlValue.Null;
        }
        var rows = steps.Select((step, i) => new[]
        {
            SqlNumbers.Number(i + 1), SqlValue.String(step.Operation), Text(step.Relation), Text(step.Detail),
            Estimate(step.EstimatedRows), Estimate(step.EstimatedCost)
        }).ToArray();
        return new(["step", "operation", "relation", "detail", "estimated_rows", "estimated_cost"], rows, $"{rows.Length} plan steps")
            { ColumnTypes = ["INTEGER", "TEXT", "TEXT", "TEXT", "REAL", "REAL"] };
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
                await ExplainMaterializationAsync(source, steps, "Materialize join input");
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
            await ExplainMaterializationAsync(source, steps, "Materialize query input");
        }
        RunQuery(query, inputs, null, describe: true, finalPage: true, explain: steps);
    }

    private async Task ExplainMaterializationAsync(SqlTable source, List<PlanStep> steps, string detail)
    {
        if (source.ViewQuery is null)
        {
            var rows = EstimateTableRows(source);
            steps.Add(new("TABLE SCAN", source.Name, detail, rows, 1d + rows));
            return;
        }
        await ExplainSelectAsync(await ReadViewQueryAsync(source), steps, baseOnly: true);
        steps.Add(new("MATERIALIZE VIEW", source.Name, detail));
    }

    private static void AppendSelectSteps(SelectRows query, List<PlanStep> steps)
    {
        if (query.Where is not null) steps.Add(new("FILTER", null, "WHERE"));
        if (query.IsGrouped) steps.Add(new("AGGREGATE", null, query.GroupBy is { } keys
            ? string.Join(", ", keys.Select(key => key.Name)) : "Single group"));
        if (query.Having is not null) steps.Add(new("FILTER", null, "HAVING"));
        foreach (var window in query.Projections.Select(p => p.Window).OfType<SqlWindow>())
            steps.Add(new("WINDOW", null, window.Function));
        AppendSortAndPage(query, steps);
    }

    private static void AppendSortAndPage(SelectRows query, List<PlanStep> steps)
    {
        if (query.Order.Count > 0) steps.Add(new("SORT", null,
            string.Join(", ", query.Order.Select(order => $"{order.Column} {(order.Descending ? "DESC" : "ASC")}"))));
        if (query.Distinct) steps.Add(new("DISTINCT", null, "Remove duplicate projected rows"));
        if (query.Offset > 0 || query.Limit.HasValue)
            steps.Add(new("LIMIT", null, $"Offset {query.Offset}, limit {query.Limit ?? MaxRows}"));
    }
}
