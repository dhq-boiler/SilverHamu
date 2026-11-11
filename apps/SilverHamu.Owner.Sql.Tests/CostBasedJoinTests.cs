using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class CostBasedJoinTests
{
    private const string Query = "SELECT a.*,b.*,c.* FROM a JOIN b ON a.shared=b.shared JOIN c ON b.id=c.id ORDER BY a.id";

    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    private static string Values(Func<int, string> row) => string.Join(",", Enumerable.Range(1, 65).Select(row));

    private static async Task<SqlDatabase> OpenJoinGraph()
    {
        var db = Open();
        await db.ExecuteAsync($"""
            CREATE TABLE a(id INT, shared INT);
            CREATE TABLE b(shared INT, id INT);
            CREATE TABLE c(id INT);
            INSERT INTO a VALUES {Values(i => $"({i},1)")};
            INSERT INTO b VALUES {Values(i => $"(1,{i})")};
            INSERT INTO c VALUES (42);
            ANALYZE a; ANALYZE b; ANALYZE c
            """);
        return db;
    }

    [Fact]
    public async Task Reordering_selective_join_avoids_an_oversized_intermediate_and_preserves_projection_order()
    {
        var db = await OpenJoinGraph();
        var plan = (await db.ExecuteAsync("EXPLAIN " + Query))[0];
        Assert.Equal("a", plan.Rows.Last(row => row[1].Text is "HASH JOIN" or "NESTED LOOP JOIN")[2].Text);
        var result = (await db.ExecuteAsync(Query))[0];
        Assert.Equal(new[] { "id", "shared", "shared", "id", "id" }, result.Columns);
        Assert.Equal(65, result.Rows.Count);
        for (int i = 0; i < result.Rows.Count; i++)
            Assert.Equal(new[] { (i + 1).ToString(), "1", "1", "42", "42" }, result.Rows[i].Select(v => v.Text));
    }

    [Fact]
    public async Task Stale_estimates_cannot_make_a_join_exceed_the_limit_if_the_written_order_fits()
    {
        var db = await OpenJoinGraph();
        await db.ExecuteAsync($"""
            DELETE FROM a; DELETE FROM b; DELETE FROM c;
            INSERT INTO a VALUES (100,42);
            INSERT INTO b VALUES {Values(i => $"({i},1)")};
            INSERT INTO c VALUES {Values(_ => "(1)")};
            """);
        var result = (await db.ExecuteAsync(Query))[0];
        Assert.Equal(65, result.Rows.Count);
        Assert.All(result.Rows, row => Assert.Equal(new[] { "100", "42", "42", "1", "1" }, row.Select(v => v.Text)));
    }

    [Fact]
    public async Task Reanalysis_changes_physical_join_strategy_for_a_small_probe_set()
    {
        var db = Open();
        await db.ExecuteAsync($"""
            CREATE TABLE a(id INT); CREATE TABLE b(id INT);
            INSERT INTO a VALUES {Values(i => $"({i})")}; INSERT INTO b VALUES {Values(i => $"({i})")};
            ANALYZE a; ANALYZE b
            """);
        const string query = "SELECT a.id FROM a JOIN b ON a.id=b.id ORDER BY a.id";
        var plan = (await db.ExecuteAsync("EXPLAIN " + query))[0];
        var hash = Assert.Single(plan.Rows, row => row[1].Text == "HASH JOIN");
        Assert.Equal("65", hash[4].Text);
        Assert.Equal(65, (await db.ExecuteAsync(query))[0].Rows.Count);
        await db.ExecuteAsync("DELETE FROM a WHERE id<>21; ANALYZE a");
        var newPlan = (await db.ExecuteAsync("EXPLAIN " + query))[0];
        var nested = Assert.Single(newPlan.Rows, row => row[1].Text == "NESTED LOOP JOIN");
        Assert.Equal("1", nested[4].Text);
        Assert.Equal("21", Assert.Single(Assert.Single((await db.ExecuteAsync(query))[0].Rows)).Text);
    }
}
