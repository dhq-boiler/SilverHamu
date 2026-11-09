using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class JoinWindowTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Using_coalesces_only_the_unqualified_keys_of_outer_joins()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE a (x TEXT, k INT, n INT); CREATE TABLE b (k INT, n INT, y TEXT);
            INSERT INTO a VALUES ('first',1,1),('left',2,1);
            INSERT INTO b VALUES (1,1,'both'),(2,9,'other'),(3,1,'right');
            """);
        var sql = "SELECT * FROM a FULL JOIN b USING (k,n) ORDER BY k,n";
        var joined = (await db.ExecuteAsync(sql))[0];
        Assert.Equal(new[] { "k", "n", "x", "y" }, joined.Columns);
        Assert.Equal(new[] { "1,1,first,both", "2,1,left,NULL", "2,9,NULL,other", "3,1,NULL,right" },
            joined.Rows.Select(r => string.Join(",", r.Select(v => v.Display))));
        var natural = (await db.ExecuteAsync("SELECT a.k,b.k,k FROM a NATURAL RIGHT JOIN b ORDER BY b.k"))[0];
        Assert.Equal(new[] { "1,1,1", "NULL,2,2", "NULL,3,3" },
            natural.Rows.Select(r => string.Join(",", r.Select(v => v.Display))));
        var cross = (await db.ExecuteAsync("SELECT COUNT(*) FROM a CROSS JOIN b"))[0];
        Assert.Equal("6", Assert.Single(cross.Rows)[0].Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM a JOIN b USING(missing)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM a CROSS JOIN b ON a.k=b.k"));
        await db.ExecuteAsync("""
            CREATE TABLE decimals(k REAL); INSERT INTO decimals VALUES (7.5);
            CREATE TABLE combined AS SELECT k FROM a FULL JOIN decimals USING(k)
            """);
        Assert.Equal("7.5", (await db.ExecuteAsync("SELECT MAX(k) FROM combined"))[0].Rows[0][0].Display);
    }

    [Fact]
    public async Task Distinct_precedes_standard_pagination_and_treats_nulls_and_equal_numbers_as_duplicates()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t (k REAL); INSERT INTO t VALUES (1),(1.00),(2),(NULL),(NULL),(3)");
        var rows = (await db.ExecuteAsync("SELECT DISTINCT k FROM t ORDER BY k OFFSET 1 ROW FETCH NEXT 2 ROWS ONLY"))[0].Rows;
        Assert.Equal(new[] { 1m, 2m }, rows.Select(r => decimal.Parse(r[0].Text!, System.Globalization.CultureInfo.InvariantCulture)));
        Assert.Empty((await db.ExecuteAsync("SELECT DISTINCT k FROM t FETCH FIRST 0 ROWS ONLY"))[0].Rows);
    }

    [Fact]
    public async Task Windows_respect_partitions_peers_frames_and_query_pagination()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE t (team TEXT, score INT, id INT PRIMARY KEY);
            INSERT INTO t VALUES ('a',5,1),('a',5,2),('a',8,3),('a',NULL,4),('b',9,5)
            """);
        var query = """
            SELECT id, RANK() OVER (PARTITION BY team ORDER BY score) AS r,
                   DENSE_RANK() OVER (PARTITION BY team ORDER BY score) AS d,
                   SUM(score) OVER (PARTITION BY team ORDER BY score) AS running,
                   COUNT(score) OVER (PARTITION BY team) AS n,
                   MIN(score) OVER (PARTITION BY team ORDER BY id ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) AS m,
                   ROW_NUMBER() OVER (PARTITION BY team ORDER BY id DESC) AS pos
            FROM t ORDER BY id OFFSET 1 ROWS FETCH NEXT 4 ROWS ONLY
            """;
        Assert.Equal(new[] { "2,2,2,10,3,5,3", "3,4,3,18,3,5,2", "4,1,1,NULL,3,8,1", "5,1,1,9,1,9,1" },
            (await db.ExecuteAsync(query))[0].Rows.Select(r => string.Join(",", r.Select(v => v.Display))));
        var navigation = (await db.ExecuteAsync("""
            SELECT LAG(score,1,-1) OVER (PARTITION BY team ORDER BY id),
                   LEAD(score) OVER (PARTITION BY team ORDER BY id),
                   FIRST_VALUE(score) OVER (PARTITION BY team ORDER BY id),
                   LAST_VALUE(score) OVER (PARTITION BY team ORDER BY id ROWS BETWEEN CURRENT ROW AND 1 FOLLOWING),
                   NTILE(3) OVER (PARTITION BY team ORDER BY id)
            FROM t ORDER BY id
            """))[0];
        Assert.Equal(new[] { "-1,5,5,5,1", "5,8,5,8,1", "5,NULL,5,NULL,2", "8,NULL,5,NULL,3", "-1,NULL,9,9,1" },
            navigation.Rows.Select(r => string.Join(",", r.Select(v => v.Display))));
    }
}
