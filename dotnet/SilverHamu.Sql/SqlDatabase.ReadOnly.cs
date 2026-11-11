using System.Globalization;

namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    public static async IAsyncEnumerable<SqlResult> ReadAllReadOnlyAsync(SelectRows query,
        Func<string, SqlTable> schema, Func<string, IAsyncEnumerable<SqlRow>> scan,
        Func<string, long?>? rowCount = null)
    {
        if (query.Table is null || query.HasQuerySources || query.Joins().Any() || query.IsGrouped ||
            query.Distinct || query.Order.Count != 0 || query.Projections.Any(p => p.Window is not null))
        {
            yield return await QueryReadOnlyAsync(query with { Limit = query.Limit ?? int.MaxValue }, schema, scan, rowCount);
            yield break;
        }

        var table = schema(query.Table);
        var bound = BindSingleSource(query, table);
        var page = bound with { Where = null, Offset = 0, Limit = 100 };
        yield return Select(bound, table, [], describeOnly: true);
        if (bound.Limit == 0) yield break;
        long skipped = 0, emitted = 0;
        var rows = new List<SqlRow>(100);
        await foreach (var row in scan(query.Table))
        {
            if (!Matches(bound.Where, row.Values)) continue;
            if (skipped < bound.Offset) { skipped++; continue; }
            rows.Add(row);
            emitted++;
            if (rows.Count == 100)
            {
                yield return Select(page, table, rows);
                rows.Clear();
            }
            if (bound.Limit is { } limit && emitted >= limit) break;
        }
        if (rows.Count > 0) yield return Select(page, table, rows);
    }

    public static Task<SqlResult> QueryReadOnlyAsync(SelectRows query, Func<string, SqlTable> schema,
        Func<string, IAsyncEnumerable<SqlRow>> scan, Func<string, long?>? rowCount = null) =>
        ExecuteReadOnlyAsync(query, schema, scan, rowCount, null);

    public static async Task<SqlResult> ExplainReadOnlyAsync(ExplainQuery explain, Func<string, SqlTable> schema,
        Func<string, IAsyncEnumerable<SqlRow>> scan, Func<string, long?>? rowCount = null)
    {
        var profile = new QueryProfile();
        if (!explain.Analyze) profile = profile.Planning();
        var started = profile.Start();
        var result = await ExecuteReadOnlyAsync(explain.Query, schema, scan, rowCount, profile);
        if (!explain.Analyze)
            return FormatPlan(explain.Query, profile.Measurements.Select(s => s.Step).ToArray());
        profile.Record(new("RESULT", null, "SELECT execution, including child operations"), result.Rows.Count, started);
        return FormatProfile(explain.Query, profile);
    }

    private static async Task<SqlResult> ExecuteReadOnlyAsync(SelectRows query, Func<string, SqlTable> schema,
        Func<string, IAsyncEnumerable<SqlRow>> scan, Func<string, long?>? rowCount, QueryProfile? profile)
    {
        bool planning = profile?.IsPlanning == true;
        long retained = 0;
        void Retain(List<SqlRow> list, SqlRow row)
        {
            retained += 128 + row.Values.Sum(p => 96L + 2L * (p.Value.Text?.Length ?? 0));
            if (retained > 128L * 1024 * 1024) throw new SqlException("書庫クエリの中間結果が128 MiBを超えました。");
            list.Add(row);
        }

        async Task<(SqlTable Table, List<SqlRow> Rows)> Materialize(string name)
        {
            var table = WithArchiveCount(schema(name), rowCount?.Invoke(name));
            var rows = new List<SqlRow>();
            var step = ArchiveScan(table, "Materialize query input");
            var input = profile?.Child(name);
            input?.Declare(step);
            if (!planning)
                await foreach (var row in ReadArchiveRows(name, scan, step, input)) Retain(rows, row);
            return (table, rows);
        }

        if (query.HasQuerySources)
            return await QueryRelationsAsync(query, Materialize, profile);
        if (query.Join is not null)
            return await SelectJoinAsync(query, Materialize, profile);
        if (query.Table is null)
        {
            var start = profile?.Start() ?? 0;
            var empty = new SqlTable("", []);
            List<SqlRow> unit = planning ? [] : [new("", new(StringComparer.OrdinalIgnoreCase))];
            profile?.Record(new("CONSTANT ROW", null, null, 1, 0), unit.Count, start);
            return Select(BindSingleSource(query, empty), empty, unit, describeOnly: planning, profile: profile);
        }

        var table = schema(query.Table);
        var bound = BindSingleSource(query, table);
        // Validate even when metadata supplies the result or LIMIT 0 skips the input.
        Select(bound, table, [], describeOnly: true);
        bool countOnly = bound.Projections is [{ Count: true }] && bound.GroupBy is not { Count: > 0 } && bound.Having is null;
        bool countFromHeader = countOnly && bound.Where is null;
        long metadataStarted = profile?.Start() ?? 0;
        long? known = countFromHeader || profile is not null ? rowCount?.Invoke(query.Table) : null;
        table = WithArchiveCount(table, known);
        if (countFromHeader && known is { } metadataCount)
        {
            profile?.Record(new("ARCHIVE ROW COUNT", table.Name, "COUNT(*) from the archive manifest", 1, 1), 1, metadataStarted);
            return ReadOnlyCountResult(bound, table, metadataCount, profile);
        }

        var scanStep = ArchiveScan(table, "Stream source rows");
        if (profile is not null) scanStep = DescribeAccess(new TableAccess(scanStep), bound, table);
        profile?.Declare(scanStep);
        var filter = profile is not null && bound.Where is not null
            ? new OperatorTiming(profile, new("FILTER", null, "WHERE")) : null;
        long count = 0;
        var selected = new List<SqlRow>();
        long started = profile?.Start() ?? 0;
        bool read = !planning && bound.Limit != 0;
        if (read)
        {
            bool stopEarly = !bound.IsGrouped && !bound.Distinct && bound.Projections.All(p => p.Window is null)
                && bound.Order.Count == 0;
            filter?.End(filter.Start(), 0);
            await foreach (var row in ReadArchiveRows(query.Table, scan, scanStep, profile))
            {
                long filtered = filter?.Start() ?? 0;
                bool matches = Matches(bound.Where, row.Values);
                filter?.End(filtered, matches ? 1 : 0);
                if (!matches) continue;
                count++;
                if (countOnly) continue;
                Retain(selected, row);
                if (stopEarly && count >= (long)bound.Offset + (bound.Limit ?? int.MaxValue)) break;
            }
        }
        filter?.Publish();
        if (countOnly)
        {
            var step = new PlanStep("AGGREGATE", table.Name, "Streaming COUNT(*)", 1);
            if (read) profile?.Record(step, 1, started); else profile?.Declare(step);
            return ReadOnlyCountResult(bound, table, count, profile);
        }
        var unfiltered = bound with { Where = null, Limit = bound.Limit ?? int.MaxValue,
            ProfileSource = profile is null ? null : query with { Where = null } };
        return Select(unfiltered, table, selected, describeOnly: planning, profile: profile);
    }

    private static PlanStep ArchiveScan(SqlTable table, string detail)
    {
        long rows = EstimateTableRows(table);
        return new("ARCHIVE SCAN", table.Name, detail, rows, 1d + rows);
    }

    private static SqlTable WithArchiveCount(SqlTable table, long? count) =>
        count is { } total && table.Statistics?.RowCount != total
            ? table with { Statistics = new(total, new(StringComparer.OrdinalIgnoreCase)) }
            : table;

    private static IAsyncEnumerable<SqlRow> ReadArchiveRows(string name, Func<string, IAsyncEnumerable<SqlRow>> scan,
        PlanStep scanStep, QueryProfile? profile) =>
        profile is null ? scan(name) : ObserveArchiveScan(name, scan, scanStep, profile);

    private static async IAsyncEnumerable<SqlRow> ObserveArchiveScan(string name, Func<string, IAsyncEnumerable<SqlRow>> scan,
        PlanStep scanStep, QueryProfile profile)
    {
        var timing = new OperatorTiming(profile, scanStep);
        var start = timing.Start();
        var reader = scan(name).GetAsyncEnumerator();
        timing.End(start, 0);
        try
        {
            while (true)
            {
                start = timing.Start();
                bool next = await reader.MoveNextAsync();
                timing.End(start, next ? 1 : 0);
                if (!next) yield break;
                yield return reader.Current;
            }
        }
        finally
        {
            start = timing.Start();
            await reader.DisposeAsync();
            timing.End(start, 0);
            timing.Publish();
        }
    }

    private static SqlResult ReadOnlyCountResult(SelectRows bound, SqlTable table, long count, QueryProfile? profile)
    {
        long start = profile?.Start() ?? 0;
        var rows = bound.Offset > 0 || bound.Limit == 0 || profile?.IsPlanning == true
            ? Array.Empty<SqlValue[]>() : [new[] { SqlValue.Number(count.ToString(CultureInfo.InvariantCulture)) }];
        if (bound.Offset > 0 || bound.Limit.HasValue)
            profile?.Record(new("LIMIT", null, $"Offset {bound.Offset}, limit {bound.Limit ?? MaxRows}"), rows.Length, start);
        return new([bound.Projections[0].Alias ?? "count"], rows, $"{table.Name}: {count} 行") { ColumnTypes = ["INTEGER"] };
    }
}
