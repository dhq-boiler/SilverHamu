using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ExplainTests
{
    [Fact]
    public async Task Explain_describes_lookup_and_sort_without_reading_rows_or_evaluating_the_projection()
    {
        var store = new MemorySqlStore();
        bool planning = false;
        var dataKeys = new[] { "row/", "pk/", "index/" }.Select(suffix => SqlNames.Hex("_catdb_owner/sql/v1/" + suffix)).ToArray();
        var db = new SqlDatabase(request =>
        {
            if (planning)
            {
                var op = request["op"]!.ToString();
                Assert.DoesNotContain(op, new[] { "put", "delete", "commit" });
                var key = (request["key"] ?? request["start"])?.ToString();
                Assert.False(key is not null && dataKeys.Any(prefix => key.StartsWith(prefix, StringComparison.Ordinal)));
            }
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE items (id INT PRIMARY KEY, price INT); INSERT INTO items VALUES (1,0)");
        planning = true;
        var plan = Assert.Single(await db.ExecuteAsync("EXPLAIN SELECT 1 / price FROM items WHERE id=1 ORDER BY price LIMIT 1"));
        Assert.Equal(new[] { "step", "operation", "relation", "detail", "estimated_rows", "estimated_cost" }, plan.Columns);
        Assert.Equal(new[] { "PRIMARY KEY LOOKUP", "FILTER", "SORT", "LIMIT" }, plan.Rows.Select(row => row[1].Text));
        Assert.Equal("items", plan.Rows[0][2].Text);
        Assert.Equal("1", plan.Rows[0][4].Text);
        Assert.False(store.InTransaction);
        var distinct = Assert.Single(await db.ExecuteAsync("EXPLAIN SELECT DISTINCT 1 AS n ORDER BY n DESC LIMIT 1"));
        Assert.Equal(new[] { "CONSTANT ROW", "SORT", "DISTINCT", "LIMIT" }, distinct.Rows.Select(row => row[1].Text));
    }

    [Fact]
    public async Task View_and_CTE_plans_show_which_inputs_are_materialized()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("""
            CREATE TABLE items(id INTEGER PRIMARY KEY);
            CREATE VIEW featured AS SELECT id FROM items WHERE id=42
            """);
        var plan = Assert.Single(await db.ExecuteAsync(
            "EXPLAIN WITH selected AS (SELECT id FROM featured) SELECT id FROM selected UNION SELECT 99"));
        var steps = plan.Rows.Select(row => (Operation: row[1].Text, Relation: row[2].Text)).ToArray();
        Assert.Contains(("PRIMARY KEY LOOKUP", "items"), steps);
        Assert.Contains(("MATERIALIZE VIEW", "featured"), steps);
        Assert.Contains(("MATERIALIZE CTE", "selected"), steps);
        Assert.Contains(("UNION", null), steps);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXPLAIN WITH t AS (SELECT missing FROM items) SELECT * FROM t"));
    }

    [Fact]
    public async Task Explain_accepts_only_queries_and_cannot_disclose_base_tables_to_a_view_role()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE items(id INT); CREATE VIEW published AS SELECT id FROM items; GRANT SELECT ON published TO reader");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXPLAIN DELETE FROM items"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXPLAIN SELECT bad FROM items"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("EXPLAIN SELECT id FROM items", "reader"));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("EXPLAIN SELECT id FROM published", "reader"));
        Assert.Single(await db.ExecuteAsync("EXPLAIN SELECT 1 / 0"));
    }

    [Fact]
    public async Task Cte_and_correlated_subquery_joins_expose_their_planned_strategies_over_shared_inputs()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items VALUES (1); ANALYZE items");
        const string query = """
            WITH pairs AS (
                SELECT a.id FROM (SELECT id FROM items) AS a
                JOIN items b ON a.id = b.id
            )
            SELECT p.id FROM pairs p WHERE EXISTS (
                SELECT DISTINCT x.id FROM items x JOIN items y ON x.id = y.id WHERE x.id = p.id ORDER BY x.id DESC
            )
            """;
        var plan = Assert.Single(await db.ExecuteAsync("EXPLAIN " + query));
        Assert.Single(plan.Rows, row => row[1].Text == "TABLE SCAN" && row[2].Text == "items");
        var joins = plan.Rows.Where(row => row[1].Text is "HASH JOIN" or "NESTED LOOP JOIN").ToList();
        Assert.Equal(2, joins.Count);
        Assert.All(joins, step => Assert.False(step[5].IsNull));
        Assert.Contains(plan.Rows, row => row[1].Text == "MATERIALIZE DERIVED TABLE" && row[2].Text == "a");
        Assert.Contains(plan.Rows, row => row[1].Text == "SUBQUERY" && row[3].Text!.Contains("per outer row"));
        Assert.DoesNotContain(plan.Rows, row => row[1].Text is "SORT" or "DISTINCT");
        Assert.Contains(plan.Rows, row => row[1].Text == "LIMIT" && row[3].Text == "Offset 0, limit 1");
        var result = Assert.Single(await db.ExecuteAsync(query));
        Assert.Equal("1", Assert.Single(Assert.Single(result.Rows)).Text);
    }

    [Fact]
    public async Task Self_joining_a_view_explains_its_underlying_access_once()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE items(id INTEGER PRIMARY KEY); CREATE VIEW featured AS SELECT id FROM items WHERE id=42");
        var plan = Assert.Single(await db.ExecuteAsync("EXPLAIN SELECT a.id FROM featured a JOIN featured b ON a.id=b.id"));
        Assert.Single(plan.Rows, row => row[1].Text == "MATERIALIZE VIEW" && row[2].Text == "featured");
        Assert.Single(plan.Rows, row => row[1].Text == "PRIMARY KEY LOOKUP" && row[2].Text == "items");
    }
}
