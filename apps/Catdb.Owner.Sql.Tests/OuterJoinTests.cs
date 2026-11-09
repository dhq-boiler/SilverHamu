using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class OuterJoinTests
{
    private static async Task<SqlDatabase> CreateAsync()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE l(k INT, label TEXT); CREATE TABLE r(k REAL, label TEXT)");
        return db;
    }

    [Theory]
    [InlineData("LEFT", "a:one,a:two,b:NULL,c:NULL")]
    [InlineData("RIGHT OUTER", "NULL:extra,NULL:unknown,a:one,a:two")]
    [InlineData("FULL", "NULL:extra,NULL:unknown,a:one,a:two,b:NULL,c:NULL")]
    public async Task Outer_join_preserves_only_the_requested_sides_and_all_duplicate_matches(string kind, string expected)
    {
        var db = await CreateAsync();
        await db.ExecuteAsync("INSERT INTO l VALUES (1,'a'),(2,'b'),(NULL,'c'); INSERT INTO r VALUES (1.0,'one'),(1.00,'two'),(3,'extra'),(NULL,'unknown')");
        var query = $"SELECT l.label, r.label FROM l {kind} JOIN r ON l.k=r.k ORDER BY l.label,r.label";
        var result = (await db.ExecuteAsync(query))[0];
        Assert.Equal(expected, string.Join(",", result.Rows.Select(row => row[0].Display + ":" + row[1].Display)));
    }

    [Fact]
    public async Task Chained_joins_can_use_any_preceding_source_and_pad_all_absent_sources()
    {
        var db = await CreateAsync();
        await db.ExecuteAsync("CREATE TABLE prices(k INT, price INT); INSERT INTO l VALUES (1,'a'),(2,'b'); INSERT INTO r VALUES (1,'x'); INSERT INTO prices VALUES (2,20),(3,30)");
        var result = (await db.ExecuteAsync("""
            SELECT a.label AS a, b.label AS b, p.price FROM l a LEFT JOIN r b ON b.k=a.k
            FULL JOIN prices p ON p.k=a.k ORDER BY p.price
            """))[0];
        Assert.Equal(new[] { "a,x,NULL", "b,NULL,20", "NULL,NULL,30" },
            result.Rows.Select(row => string.Join(",", row.Select(v => v.Display))));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM l JOIN r ON l.k=prices.k JOIN prices ON prices.k=r.k"));
        var oneSided = (await db.ExecuteAsync("SELECT COUNT(*) FROM l JOIN r ON l.k=r.k JOIN prices ON l.k=r.k"))[0];
        Assert.Equal("2", oneSided.Rows.Single()[0].Display);
    }

    [Fact]
    public async Task Joined_view_tracks_every_base_table_and_exposes_only_public_result()
    {
        var db = await CreateAsync();
        await db.ExecuteAsync("""
            CREATE TABLE flags(k INT, visible BOOLEAN);
            INSERT INTO l VALUES (1,'first'),(2,'second'); INSERT INTO r VALUES (1,'one'),(2,'two');
            INSERT INTO flags VALUES (1,TRUE),(2,FALSE);
            CREATE VIEW labels AS SELECT l.label AS name, r.label AS tag FROM l JOIN r ON l.k=r.k
            JOIN flags f ON f.k=l.k WHERE f.visible=TRUE;
            """);
        var published = await db.QueryPublishedViewAsync("SELECT * FROM labels", ["labels"]);
        Assert.Equal(new[] { "first", "one" }, Assert.Single(published.Rows).Select(v => v.Display));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryPublishedViewAsync("SELECT visible FROM labels", ["labels"]));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DROP TABLE flags"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE flags RENAME COLUMN visible TO public_flag"));
        await db.ExecuteAsync("ALTER TABLE flags ADD COLUMN extra TEXT; UPDATE flags SET visible=TRUE");
        Assert.Equal(2, (await db.ExecuteAsync("SELECT * FROM labels"))[0].Rows.Count);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE VIEW indirect AS SELECT v.name AS name FROM labels v JOIN l ON v.name=l.label"));
        await db.ExecuteAsync("DROP VIEW labels; DROP TABLE flags");
    }

    [Fact]
    public async Task Outer_join_extends_empty_sides_and_filters_the_extended_rows_before_aggregation()
    {
        var db = await CreateAsync();
        const string query = "SELECT COUNT(*) FROM l FULL OUTER JOIN r ON r.k=l.k WHERE l.label IS NULL";
        Assert.Equal("0", (await db.ExecuteAsync(query))[0].Rows[0][0].Display);
        await db.ExecuteAsync("INSERT INTO r VALUES (0,'zero'),(1,'one')");
        Assert.Equal("2", (await db.ExecuteAsync(query))[0].Rows[0][0].Display);
        await db.ExecuteAsync("INSERT INTO l VALUES (0,'0'),(2,'2')");
        Assert.Equal("1", (await db.ExecuteAsync(query))[0].Rows[0][0].Display);
        await db.ExecuteAsync("DELETE FROM r");
        Assert.Equal("2", (await db.ExecuteAsync("SELECT COUNT(*) FROM l LEFT OUTER JOIN r ON l.k=r.k WHERE r.label IS NULL"))[0].Rows[0][0].Display);
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM l RIGHT JOIN r ON l.k=r.k"))[0].Rows);
    }
}
