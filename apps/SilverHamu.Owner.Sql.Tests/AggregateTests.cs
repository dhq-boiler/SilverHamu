using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class AggregateTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Grouping_uses_numeric_equality_and_nulls_and_sorts_after_aggregation()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE sales(k REAL, dept TEXT, amount REAL);
            INSERT INTO sales VALUES (1,'a',10),(1.00,'a',20),(NULL,'a',NULL),(NULL,'a',6),(1,'b',3),(2,'a',NULL);
            """);
        var result = (await db.ExecuteAsync("""
            SELECT dept, COUNT(*) AS n, SUM(amount) AS total, AVG(amount) AS mean
            FROM sales GROUP BY k, dept ORDER BY total DESC LIMIT 3
            """))[0];
        Assert.Equal(new[] { "dept", "n", "total", "mean" }, result.Columns);
        Assert.Equal(new[] { "a,2,30,15", "a,2,6,6", "b,1,3,3" },
            result.Rows.Select(row => string.Join(",", row.Select(v => v.Display))));
        Assert.Equal(4, (await db.ExecuteAsync("SELECT k FROM sales GROUP BY k, dept ORDER BY k, dept"))[0].Rows.Count);
    }

    [Fact]
    public async Task Empty_inputs_and_all_null_groups_follow_aggregate_semantics()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t(i INTEGER); INSERT INTO t VALUES (NULL),(NULL)");
        foreach (var (where, count) in new[] { ("", "2"), (" WHERE i=0", "0") })
        {
            var row = Assert.Single((await db.ExecuteAsync("SELECT COUNT(*), SUM(i), AVG(i) FROM t" + where))[0].Rows);
            Assert.Equal(count, row[0].Display);
            Assert.True(row[1].IsNull);
            Assert.True(row[2].IsNull);
        }
        Assert.Empty((await db.ExecuteAsync("SELECT COUNT(*) FROM t WHERE i=0 GROUP BY i"))[0].Rows);
    }

    [Fact]
    public async Task Qualified_grouping_works_after_join_and_aggregate_view_reads_current_data()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE items(id INT, cost REAL); CREATE TABLE names(id INT, title TEXT);
            INSERT INTO items VALUES (1,6),(1,2),(2,7); INSERT INTO names VALUES (1,'猫'),(2,'本');
            CREATE VIEW totals AS SELECT id, AVG(cost) AS mean FROM items GROUP BY id;
            """);
        var result = (await db.ExecuteAsync("SELECT n.title, SUM(i.cost) AS total FROM items i JOIN names n ON i.id=n.id GROUP BY n.title ORDER BY total DESC"))[0];
        Assert.Equal(new[] { "猫,8", "本,7" }, result.Rows.Select(r => r[0].Display + "," + r[1].Display));
        await db.ExecuteAsync("INSERT INTO items VALUES (1,4)");
        Assert.Equal("4", Assert.Single((await db.ExecuteAsync("SELECT mean FROM totals WHERE id=1"))[0].Rows)[0].Display);
    }

    [Theory]
    [InlineData("SELECT id, SUM(i) FROM t")]
    [InlineData("SELECT id, SUM(i) FROM t GROUP BY i")]
    [InlineData("SELECT SUM(name) FROM t")]
    [InlineData("SELECT AVG(missing) FROM t")]
    [InlineData("SELECT COUNT(*) FROM t GROUP BY missing")]
    [InlineData("SELECT COUNT(*) FROM t GROUP BY i ORDER BY id")]
    public async Task Invalid_aggregate_references_fail_even_without_rows(string sql)
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t(id INT, i INT, name TEXT)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(sql));
    }
}
