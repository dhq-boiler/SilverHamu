using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class CompoundJoinTests
{
    private static SqlDatabase Database()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Theory]
    [InlineData("INNER", "A:X,A:Y")]
    [InlineData("LEFT", "A:X,A:Y,B:NULL,C:NULL,D:NULL")]
    [InlineData("RIGHT", "NULL:N,NULL:U,NULL:W,NULL:Z,A:X,A:Y")]
    [InlineData("FULL", "NULL:N,NULL:U,NULL:W,NULL:Z,A:X,A:Y,B:NULL,C:NULL,D:NULL")]
    public async Task Each_candidate_must_match_all_keys_and_pass_the_residual_before_outer_padding(string kind, string expected)
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE l(tenant INTEGER, id INTEGER, price REAL, label TEXT);
            CREATE TABLE r(tenant INTEGER, id INTEGER, price REAL, active BOOLEAN, label TEXT);
            INSERT INTO l VALUES (1,1,10,'A'),(1,1,100,'B'),(NULL,1,0,'C'),(1,2,20,'D');
            INSERT INTO r VALUES (1,1,11,TRUE,'X'),(1,1,12,TRUE,'Y'),(1,1,8,TRUE,'Z'),
                (2,1,1000,TRUE,'W'),(NULL,1,1200,TRUE,'N'),(1,2,99,NULL,'U');
            """);
        var result = (await db.ExecuteAsync($"""
            SELECT l.label, r.label FROM l {kind} JOIN r ON
              (l.id=r.id AND r.tenant=l.tenant) AND (r.active=TRUE AND l.price<r.price)
            ORDER BY l.label, r.label
            """))[0];
        Assert.Equal(expected, string.Join(",", result.Rows.Select(row => $"{row[0].Display}:{row[1].Display}")));
    }

    [Fact]
    public async Task Keys_in_a_stored_join_can_refer_to_different_preceding_inputs()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE a(id INTEGER, tenant INTEGER); CREATE TABLE b(id INTEGER, code TEXT);
            CREATE TABLE c(tenant INTEGER, code TEXT, label TEXT);
            INSERT INTO a VALUES (1,7),(2,8); INSERT INTO b VALUES (1,'t'),(2,'x');
            INSERT INTO c VALUES (7,'x','wrong tenant'),(7,'t','first'),(8,'x','second');
            CREATE VIEW matches AS SELECT c.label AS result FROM a JOIN b ON a.id=b.id
                JOIN c ON c.tenant=a.tenant AND b.code=c.code;
            """);
        var result = (await db.ExecuteAsync("SELECT result FROM matches ORDER BY result"))[0];
        Assert.Equal(new[] { "first", "second" }, result.Rows.Select(r => r[0].Display));
    }

    [Theory]
    [InlineData("a.id=b.id AND a.tenant=b.label")]
    [InlineData("a.id=b.id AND b.label")]
    [InlineData("a.id=b.id AND b.missing=1")]
    [InlineData("a.id=b.id AND later.id=a.id")]
    [InlineData("a.id=b.id OR b.label")]
    public async Task Invalid_shapes_and_types_are_rejected_with_empty_inputs(string condition)
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE a(id INT, tenant INT); CREATE TABLE b(id INT, tenant INT, label TEXT)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"SELECT a.id FROM a JOIN b ON {condition}"));
    }
}
