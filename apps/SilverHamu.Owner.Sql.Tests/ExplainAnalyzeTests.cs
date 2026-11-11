using System.Globalization;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ExplainAnalyzeTests
{
    private static SqlValue Cell(SqlResult result, SqlValue[] row, string column) =>
        row[result.Columns.ToList().IndexOf(column)];

    private static SqlValue[] Step(SqlResult result, string name) =>
        Assert.Single(result.Rows, row => Cell(result, row, "operation").Text == name);

    private static decimal Number(SqlResult result, string step, string column) =>
        decimal.Parse(Cell(result, Step(result, step), column).Text!, CultureInfo.InvariantCulture);

    [Fact]
    public async Task A_single_scan_reports_stale_estimates_actual_output_and_time_spent_waiting_for_storage()
    {
        var store = new MemorySqlStore();
        bool profiling = false;
        int scans = 0;
        var prefix = SqlNames.Hex("_catdb_owner/sql/v1/row/");
        var db = new SqlDatabase(async request =>
        {
            if (profiling && request["op"]?.ToString() == "scan" &&
                request["start"]!.ToString().StartsWith(prefix, StringComparison.Ordinal))
            {
                scans++;
                await Task.Delay(30);
            }
            return await store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE sales (amount INT); INSERT INTO sales VALUES (10),(20); ANALYZE sales; INSERT INTO sales VALUES (30),(40)");
        profiling = true;
        var result = Assert.Single(await db.ExecuteAsync("EXPLAIN ANALYZE SELECT amount FROM sales WHERE amount >= 20 ORDER BY amount LIMIT 2"));
        Assert.Equal(1, scans);
        Assert.Equal(2, Number(result, "TABLE SCAN", "estimated_rows"));
        Assert.Equal(4, Number(result, "TABLE SCAN", "actual_rows"));
        Assert.Equal(1, Number(result, "TABLE SCAN", "loops"));
        Assert.True(Number(result, "TABLE SCAN", "elapsed_ms") >= 15);
        Assert.Equal(1, Number(result, "FILTER", "estimated_rows"));
        Assert.Equal(3, Number(result, "FILTER", "actual_rows"));
        Assert.Equal(2, Number(result, "LIMIT", "actual_rows"));
        Assert.Equal(2, Number(result, "RESULT", "actual_rows"));
        Assert.False(store.InTransaction);
    }

    [Fact]
    public async Task Profiling_uses_the_callers_snapshot_and_propagates_runtime_errors()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t(id INT PRIMARY KEY, n INT); BEGIN; INSERT INTO t VALUES (1,0)");
        const string sql = "SELECT 10/n FROM t WHERE id=1";
        await db.ExecuteAsync("EXPLAIN " + sql);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXPLAIN ANALYZE " + sql));
        Assert.True(store.InTransaction);
        var profile = Assert.Single(await db.ExecuteAsync("EXPLAIN ANALYZE SELECT n FROM t WHERE id=1"));
        Assert.Equal(1, Number(profile, "PRIMARY KEY LOOKUP", "actual_rows"));
        await db.ExecuteAsync("ROLLBACK");
        profile = Assert.Single(await db.ExecuteAsync("EXPLAIN ANALYZE SELECT n FROM t WHERE id=1"));
        Assert.Equal(0, Number(profile, "PRIMARY KEY LOOKUP", "actual_rows"));
        Assert.Equal(0, Number(profile, "RESULT", "actual_rows"));
    }

    [Fact]
    public async Task Analyze_can_only_execute_queries_and_view_credentials_cannot_inspect_their_sources()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t(n INT); INSERT INTO t VALUES (1); CREATE VIEW v AS SELECT n FROM t; GRANT SELECT ON v TO reader");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXPLAIN ANALYZE DELETE FROM t"));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("EXPLAIN ANALYZE SELECT * FROM v", "reader"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("EXPLAIN ANALYZE SELECT * FROM t", "reader"));
        var profile = Assert.Single(await db.ExecuteAsync("EXPLAIN ANALYZE SELECT * FROM v"));
        Assert.Equal(1, Number(profile, "MATERIALIZE VIEW", "actual_rows"));
        Assert.Equal(1, Number(profile, "TABLE SCAN", "actual_rows"));
    }
}
