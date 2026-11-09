using System.Globalization;
using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class ExplainPipelineProfileTests
{
    private sealed record Stage(string? Operation, string? Detail, long Rows, long Loops, decimal Elapsed, string? Average);

    private static SqlDatabase Database()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    private static async Task<Stage[]> Profile(SqlDatabase db, string query)
    {
        var plan = Assert.Single(await db.ExecuteAsync("EXPLAIN ANALYZE " + query));
        var columns = plan.Columns.ToList();
        return plan.Rows.Select(row =>
        {
            string? Text(string name) => row[columns.IndexOf(name)].Text;
            return new Stage(Text("operation"), Text("detail"),
                long.Parse(Text("actual_rows")!, CultureInfo.InvariantCulture),
                long.Parse(Text("loops")!, CultureInfo.InvariantCulture),
                decimal.Parse(Text("elapsed_ms")!, CultureInfo.InvariantCulture),
                Text("actual_rows_per_loop"));
        }).ToArray();
    }

    [Fact]
    public async Task Having_reports_surviving_groups_and_identical_window_functions_have_separate_measurements()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE t(g INT, n INT); INSERT INTO t VALUES (1,1),(1,2),(2,1),(3,4)");
        var steps = await Profile(db, """
            SELECT g, SUM(n) AS total, ROW_NUMBER() OVER (ORDER BY g) AS asc_rank,
                ROW_NUMBER() OVER (ORDER BY g DESC) AS desc_rank
            FROM t GROUP BY g HAVING SUM(n)>1 ORDER BY g
            """);
        Assert.Equal(3, Assert.Single(steps, s => s.Operation == "AGGREGATE").Rows);
        Assert.Equal(2, Assert.Single(steps, s => s.Detail == "HAVING").Rows);
        var windows = steps.Where(s => s.Operation == "WINDOW" && s.Detail == "ROW_NUMBER").ToArray();
        Assert.Equal(2, windows.Length);
        Assert.All(windows, w => { Assert.Equal(1, w.Loops); Assert.Equal(2, w.Rows); });
        Assert.Equal(2, Assert.Single(steps, s => s.Operation == "PROJECT").Rows);
        Assert.Equal(2, Assert.Single(steps, s => s.Operation == "SORT").Rows);
    }

    [Fact]
    public async Task Distinct_projection_and_sort_stop_emitting_when_the_page_is_full()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE t(n INT); INSERT INTO t VALUES (0),(1),(1)");
        const string query = "SELECT DISTINCT 1/n AS reciprocal FROM t ORDER BY n DESC LIMIT 1";
        Assert.Equal("1", Assert.Single(Assert.Single(await db.ExecuteAsync(query)).Rows)[0].Text);
        var steps = await Profile(db, query);
        Assert.Equal(3, Assert.Single(steps, s => s.Operation == "TABLE SCAN").Rows);
        foreach (string operation in new[] { "SORT", "PROJECT", "DISTINCT", "LIMIT", "RESULT" })
            Assert.Equal(1, Assert.Single(steps, s => s.Operation == operation).Rows);
    }

    [Fact]
    public async Task A_window_on_empty_input_runs_once_but_a_window_skipped_by_limit_zero_has_no_samples()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE t(n INT)");
        const string query = "SELECT ROW_NUMBER() OVER (ORDER BY n) AS pos FROM t";
        var ran = Assert.Single(await Profile(db, query), s => s.Operation == "WINDOW");
        Assert.Equal(0, ran.Rows);
        Assert.Equal(1, ran.Loops);
        Assert.Equal("0", ran.Average);
        Assert.True(ran.Elapsed >= 0);
        var skipped = Assert.Single(await Profile(db, query + " LIMIT 0"), s => s.Operation == "WINDOW");
        Assert.Equal(0, skipped.Rows);
        Assert.Equal(0, skipped.Loops);
        Assert.Null(skipped.Average);
        Assert.Equal(0m, skipped.Elapsed);
    }
}
