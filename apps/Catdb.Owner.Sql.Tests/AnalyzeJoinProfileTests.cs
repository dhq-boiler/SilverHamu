using System.Globalization;
using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class AnalyzeJoinProfileTests
{
    private sealed record Sample(string Operation, string? Relation, string? Detail, int Rows, int Loops);

    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    private static async Task<Sample[]> Profile(SqlDatabase db, string query)
    {
        var result = (await db.ExecuteAsync("EXPLAIN ANALYZE " + query))[0];
        return result.Rows.Select(row => new Sample(row[1].Text!, row[2].Text, row[3].Text,
            int.Parse(row[6].Text!, CultureInfo.InvariantCulture), int.Parse(row[7].Text!, CultureInfo.InvariantCulture))).ToArray();
    }

    [Fact]
    public async Task Cached_and_correlated_subqueries_report_their_own_runtime_passes()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE a(id INT); CREATE TABLE b(id INT); INSERT INTO a VALUES (1),(2),(3); INSERT INTO b VALUES (1),(1),(3),(4)");
        var plan = await Profile(db, """
            SELECT (SELECT COUNT(*) FROM b WHERE b.id=a.id) AS matches, (SELECT COUNT(*) FROM b) AS total
            FROM a ORDER BY a.id
            """);
        Assert.Contains(plan, s => s.Operation == "SUBQUERY" && s.Loops == 3 && s.Rows == 3);
        Assert.Contains(plan, s => s.Operation == "SUBQUERY" && s.Loops == 1 && s.Rows == 1);
        Assert.Contains(plan, s => s.Operation == "READ MATERIALIZED INPUT" && s.Relation == "b" && s.Loops == 3 && s.Rows == 12);
        Assert.Contains(plan, s => s.Operation == "READ MATERIALIZED INPUT" && s.Relation == "b" && s.Loops == 1 && s.Rows == 4);
        Assert.Single(plan, s => s.Operation == "TABLE SCAN" && s.Relation == "b" && s.Loops == 1);
    }

    [Fact]
    public async Task Subquery_which_receives_no_outer_rows_has_zero_invocations()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE a(id INT); CREATE TABLE b(id INT); INSERT INTO a VALUES (1)");
        var plan = await Profile(db, "SELECT (SELECT COUNT(*) FROM b WHERE b.id=a.id) AS n FROM a WHERE a.id<0");
        Assert.Contains(plan, s => s.Operation == "SUBQUERY" && s.Rows == 0 && s.Loops == 0);
        Assert.Contains(plan, s => s.Operation == "READ MATERIALIZED INPUT" && s.Relation == "b" && s.Rows == 0 && s.Loops == 0);
        Assert.Contains(plan, s => s.Operation == "FILTER" && s.Rows == 0 && s.Loops == 1);
    }

    [Fact]
    public async Task Recursive_term_loops_are_separate_from_final_read_of_the_cte()
    {
        var plan = await Profile(Open(), """
            WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<3)
            SELECT * FROM seq
            """);
        Assert.Contains(plan, s => s.Operation == "RECURSIVE CTE" && s.Relation == "seq" && s.Rows == 3 && s.Loops == 1);
        Assert.Contains(plan, s => s.Operation == "READ MATERIALIZED INPUT" && s.Relation == "seq" && s.Rows == 3 && s.Loops == 3);
        Assert.Contains(plan, s => s.Operation == "READ MATERIALIZED INPUT" && s.Relation == "seq" && s.Rows == 3 && s.Loops == 1);
    }

    [Fact]
    public async Task Self_join_reads_a_view_once_and_counts_duplicate_matches()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE logs(k INT); INSERT INTO logs VALUES (1),(1),(2),(NULL); CREATE VIEW v AS SELECT k FROM logs");
        var plan = await Profile(db, "SELECT l.k FROM v l JOIN v r ON l.k=r.k");
        Assert.Contains(plan, s => s.Operation == "TABLE SCAN" && s.Relation == "logs" && s.Rows == 4 && s.Loops == 1);
        Assert.Single(plan, s => s.Operation == "MATERIALIZE VIEW" && s.Relation == "v" && s.Rows == 4 && s.Loops == 1);
        Assert.Contains(plan, s => s.Operation is "HASH JOIN" or "NESTED LOOP JOIN" && s.Rows == 5 && s.Loops == 1);
    }

    [Fact]
    public async Task Join_retry_shows_abandoned_work_and_reuses_scanned_inputs()
    {
        var db = Open();
        string Tuples(Func<int, string> row) => string.Join(',', Enumerable.Range(1, 65).Select(row));
        await db.ExecuteAsync($"""
            CREATE TABLE a(k INT); CREATE TABLE b(k INT, i INT); CREATE TABLE c(i INT);
            INSERT INTO a VALUES {Tuples(_ => "(1)")}; INSERT INTO b VALUES {Tuples(i => $"(1,{i})")};
            INSERT INTO c VALUES (42); ANALYZE a; ANALYZE b; ANALYZE c;
            DELETE FROM a; DELETE FROM b; DELETE FROM c; INSERT INTO a VALUES (42);
            INSERT INTO b VALUES {Tuples(i => $"({i},1)")}; INSERT INTO c VALUES {Tuples(_ => "(1)")};
            """);
        var plan = await Profile(db, "SELECT a.k FROM a JOIN b ON a.k=b.k JOIN c ON b.i=c.i");
        Assert.Contains(plan, s => s.Operation == "JOIN RETRY" && s.Loops == 1);
        Assert.Contains(plan, s => s.Operation is "HASH JOIN" or "NESTED LOOP JOIN" && s.Rows == 4096 && s.Loops == 1);
        Assert.Contains(plan, s => s.Operation == "RESULT" && s.Rows == 65);
        Assert.Equal(3, plan.Where(s => s.Operation == "TABLE SCAN").Sum(s => s.Loops));
    }
}
