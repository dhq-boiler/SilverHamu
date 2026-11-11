using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class SubqueryTests
{
    [Fact]
    public async Task Membership_uses_one_column_of_visible_rows_and_current_transaction()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE items(id INT); INSERT INTO items VALUES(1),(2),(3),(NULL); CREATE TABLE source(n REAL, ok BOOLEAN); INSERT INTO source VALUES(1.0,TRUE),(1.00,TRUE),(NULL,TRUE),(3,FALSE); CREATE VIEW allowed AS SELECT n FROM source WHERE ok=TRUE");
        const string query = "SELECT i.id FROM items i WHERE i.id IN (SELECT n FROM allowed) ORDER BY i.id";
        Assert.Equal("1", Assert.Single((await db.ExecuteAsync(query))[0].Rows)[0].Display);
        await db.ExecuteAsync("BEGIN; INSERT INTO source VALUES(2,TRUE)");
        Assert.Equal(new[] { "1", "2" }, (await db.ExecuteAsync(query))[0].Rows.Select(r => r[0].Display));
        await db.ExecuteAsync("ROLLBACK");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM items WHERE id IN (SELECT n FROM allowed WHERE n<0)"))[0].Rows);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM items WHERE id IN (SELECT ok FROM source WHERE n<0)"));
    }

    [Fact]
    public async Task Cannot_reach_a_hidden_table_through_role_allowlist_or_view_definition()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t(id INT); CREATE VIEW v AS SELECT id FROM t; GRANT SELECT ON v TO reader");
        const string bypass = "SELECT id FROM v WHERE id IN (SELECT id FROM t)";
        await Assert.ThrowsAsync<SqlException>(() => db.QueryPublishedViewAsync(bypass, ["v"]));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync(bypass, "reader"));
        foreach (var sql in new[] { "CREATE VIEW vv AS " + bypass, "CREATE TABLE invalid(id INT CHECK(id IN (SELECT id FROM t)))" })
            Assert.Throws<SqlException>(() => SqlParser.Parse(sql));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM t WHERE id IN (SELECT id,id FROM t)"));
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM t WHERE id IN (SELECT id FROM t WHERE id IN (SELECT id FROM t))"))[0].Rows);
    }
}
