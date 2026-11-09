using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class RoleViewTests : IDisposable
{
    private readonly string directory = Directory.CreateTempSubdirectory("catdb-role-view-").FullName;
    private string PathToDb => Path.Combine(directory, "roles.ndb");

    [Fact]
    public async Task Grants_survive_reopen_and_disappear_on_revoke_or_view_recreation()
    {
        using (var initial = await CatdbSqlConnection.OpenAsync(PathToDb, create: true))
        {
            await initial.ExecuteAsync("""
                CREATE TABLE docs(id INT, visible BOOLEAN);
                INSERT INTO docs VALUES(1,TRUE),(2,FALSE);
                CREATE VIEW published AS SELECT id FROM docs WHERE visible=TRUE;
                CREATE VIEW private_docs AS SELECT id FROM docs WHERE visible=FALSE;
                GRANT SELECT ON published TO [Sales Reader];
                GRANT SELECT ON published TO [Sales Reader];
                """);
        }
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb);
        var list = await db.QueryRoleViewAsync("SHOW VIEWS", "sales reader");
        Assert.Equal("view_name", Assert.Single(list.Columns));
        Assert.Equal("published", Assert.Single(Assert.Single(list.Rows)).Display);
        var rows = await db.QueryRoleViewAsync("SELECT id FROM published", "SALES READER");
        Assert.Equal("1", Assert.Single(Assert.Single(rows.Rows)).Display);
        Assert.Empty(await db.ListRoleViewsAsync("unrelated"));
        await db.ExecuteAsync("BEGIN; REVOKE SELECT ON published FROM [Sales Reader]; ROLLBACK");
        Assert.Single(await db.ListRoleViewsAsync("Sales Reader"));
        await db.ExecuteAsync("REVOKE SELECT ON published FROM [Sales Reader]");
        Assert.Empty(await db.ListRoleViewsAsync("Sales Reader"));
        await db.ExecuteAsync("GRANT SELECT ON published TO [Sales Reader]; DROP VIEW published; CREATE VIEW published AS SELECT visible FROM docs");
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("SELECT * FROM published", "Sales Reader"));
    }

    [Fact]
    public async Task Restricted_queries_reject_other_sources_writes_and_open_privileged_transactions()
    {
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb, create: true);
        await db.ExecuteAsync("CREATE TABLE t(id INT, secret TEXT); CREATE VIEW v AS SELECT id FROM t; GRANT SELECT ON v TO reader");
        foreach (var text in new[] { "SELECT secret FROM t", "SELECT secret FROM v", "SHOW TABLES", "GRANT SELECT ON v TO reader",
            "DROP VIEW v", "SELECT * FROM v; SELECT * FROM t", "SELECT t.secret FROM v JOIN t ON v.id=t.id",
            "SELECT * FROM v WHERE id IN (SELECT id FROM t)", "BEGIN" })
            await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync(text, "reader"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("GRANT SELECT ON t TO reader"));
        await db.ExecuteAsync("BEGIN; GRANT SELECT ON v TO second");
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("SHOW VIEWS", "second"));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("SELECT * FROM v", "second"));
        await db.ExecuteAsync("ROLLBACK");
        Assert.Empty(await db.ListRoleViewsAsync("second"));
    }

    public void Dispose() => Directory.Delete(directory, recursive: true);
}
