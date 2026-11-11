using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class QueryExpressionTests
{
    private static SqlDatabase Database()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Correlated_queries_bind_nearest_scope_and_validate_before_scanning()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE groups(id INT);
            INSERT INTO groups VALUES (1),(2),(3);
            CREATE TABLE items(gid INT, n INT);
            INSERT INTO items VALUES(1,10),(1,20),(2,30);
            """);
        var result = (await db.ExecuteAsync("""
            SELECT g.id, (SELECT MAX(i.n) FROM items i WHERE i.gid=g.id) AS biggest
            FROM groups g
            WHERE EXISTS (SELECT 1 FROM items i WHERE i.gid=g.id AND EXISTS (SELECT g.id))
            ORDER BY g.id
            """))[0];
        Assert.Equal(new[] { "1:20", "2:30" }, result.Rows.Select(r => $"{r[0].Display}:{r[1].Display}"));
        var scalar = (await db.ExecuteAsync("""
            SELECT (SELECT n FROM items WHERE gid=9) AS missing,
                   (SELECT id FROM groups x WHERE x.id=2) AS shadowed
            FROM groups x WHERE x.id=1
            """))[0];
        Assert.True(scalar.Rows[0][0].IsNull);
        Assert.Equal("2", scalar.Rows[0][1].Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT (SELECT n FROM items)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT (SELECT g.missing) FROM groups g WHERE g.id=99"));
        var conditional = (await db.ExecuteAsync("SELECT 5 NOT IN (SELECT NULL UNION ALL SELECT 7), EXISTS (SELECT 1)"))[0];
        Assert.True(conditional.Rows[0][0].IsNull);
        Assert.True(conditional.Rows[0][1].IsTrue);
        Assert.True((await db.ExecuteAsync("SELECT EXISTS (SELECT 1/0 FROM items)"))[0].Rows[0][0].IsTrue);
    }

    [Fact]
    public async Task Set_operations_use_sql_equality_precedence_and_global_pagination()
    {
        var db = Database();
        var result = (await db.ExecuteAsync("""
            SELECT n FROM (SELECT 3 AS n UNION ALL SELECT 1 UNION ALL SELECT 3) d
            UNION SELECT 2 INTERSECT SELECT 9
            EXCEPT SELECT 1
            ORDER BY n DESC OFFSET 0 ROWS FETCH NEXT 1 ROW ONLY
            """))[0];
        Assert.Equal("3", Assert.Single(result.Rows)[0].Display);
        Assert.Single((await db.ExecuteAsync("SELECT NULL UNION SELECT NULL"))[0].Rows);
        Assert.Single((await db.ExecuteAsync("SELECT 1 AS n UNION SELECT 1.00"))[0].Rows);
        Assert.Equal(2, (await db.ExecuteAsync("SELECT 1 UNION ALL SELECT 1"))[0].Rows.Count);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT 1 UNION SELECT 1,2"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT 1 UNION SELECT '1'"));
    }

    [Fact]
    public async Task Recursive_cte_joins_frontier_until_fixpoint_and_bounds_cycles()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE links(src INT,dst INT); INSERT INTO links VALUES(1,2),(2,3),(3,1)");
        var result = (await db.ExecuteAsync("""
            WITH RECURSIVE walk(id) AS (
                SELECT 1 UNION SELECT e.dst FROM links e JOIN walk w ON e.src=w.id
            ), later AS (SELECT id FROM walk WHERE id>1)
            SELECT a.id FROM later a JOIN (SELECT id FROM walk) AS b(node) ON a.id=b.node ORDER BY a.id
            """))[0];
        Assert.Equal(new[] { "2", "3" }, result.Rows.Select(r => r[0].Display));
        var series = (await db.ExecuteAsync("""
            WITH RECURSIVE seq(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM seq WHERE n<5)
            SELECT SUM(n) FROM seq
            """))[0];
        Assert.Equal("15", series.Rows[0][0].Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(
            "WITH RECURSIVE loop(n) AS (SELECT 1 UNION ALL SELECT n FROM loop) SELECT * FROM loop"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(
            "WITH RECURSIVE loop(n) AS (SELECT n FROM loop UNION SELECT 1) SELECT * FROM loop"));
    }

    [Fact]
    public async Task Archive_query_uses_scoped_sources_and_loads_each_table_once()
    {
        var schema = new SqlTable("t", [new("id","INTEGER",false,false,false,null,null)]);
        int reads = 0;
        async IAsyncEnumerable<SqlRow> Scan(string name)
        {
            Assert.Equal("t", name);
            reads++;
            await Task.CompletedTask;
            yield return new("", new() { ["id"] = SqlValue.Number("1") });
            yield return new("", new() { ["id"] = SqlValue.Number("2") });
        }
        var query = Assert.IsType<SelectRows>(SqlParser.Parse("""
            WITH c AS (SELECT id FROM t) SELECT v.id FROM (SELECT id FROM c) v
            WHERE EXISTS (SELECT 1 FROM t WHERE t.id=v.id) ORDER BY v.id DESC
            """).Single());
        var result = await SqlDatabase.QueryReadOnlyAsync(query, _ => schema, Scan);
        Assert.Equal(new[] { "2", "1" }, result.Rows.Select(row => row[0].Display));
        Assert.Equal(1, reads);
    }

    [Fact]
    public async Task Archive_intermediate_relations_never_discard_unpaged_rows()
    {
        var schema = new SqlTable("archive", [new("id", "INTEGER", false, false, false, null, null)]);
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            await Task.CompletedTask;
            for (int i = 0; i <= SqlDatabase.MaxRows; i++)
                yield return new("", new() { ["id"] = SqlValue.Number(i.ToString()) });
        }
        Task<SqlResult> Query(string sql) => SqlDatabase.QueryReadOnlyAsync(
            Assert.IsType<SelectRows>(SqlParser.Parse(sql).Single()), _ => schema, Scan);

        foreach (var sql in new[]
        {
            "SELECT COUNT(*) FROM (SELECT id FROM archive) d",
            "WITH c AS (SELECT id FROM archive) SELECT COUNT(*) FROM c",
            "SELECT 4096 IN (SELECT id FROM archive)"
        })
            Assert.Contains("4096", (await Assert.ThrowsAsync<SqlException>(() => Query(sql))).Message);
        var bounded = await Query("SELECT COUNT(*) FROM (SELECT id FROM archive LIMIT 4096) d");
        Assert.Equal("4096", bounded.Rows[0][0].Display);
        Assert.Equal(SqlDatabase.MaxRows, (await Query("SELECT id FROM archive WHERE EXISTS (SELECT 1)")).Rows.Count);
        var empty = await Query("SELECT (SELECT SUM(1/id) FROM archive WHERE 1/id>0 LIMIT 0)");
        Assert.True(empty.Rows.Single()[0].IsNull);
    }

    [Theory]
    [InlineData("WITH hidden AS (SELECT id FROM t) SELECT id FROM v")]
    [InlineData("SELECT (SELECT id FROM t) FROM v")]
    [InlineData("SELECT id FROM v WHERE EXISTS (SELECT 1 FROM t)")]
    [InlineData("SELECT id FROM (SELECT id FROM t) v")]
    [InlineData("SELECT id FROM v UNION SELECT id FROM t")]
    public async Task Published_and_role_views_cannot_introduce_another_query(string sql)
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE t(id INT); CREATE VIEW v AS SELECT id FROM t; GRANT SELECT ON v TO reader");
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync(sql, "reader"));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryPublishedViewAsync(sql, ["v"]));
    }
}
