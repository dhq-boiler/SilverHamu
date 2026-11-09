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
            "SELECT * FROM v WHERE id IN (SELECT id FROM t)",
            "SELECT COUNT(*) FROM v HAVING (SELECT id FROM t)=1",
            "SELECT SUM((SELECT id FROM t)) OVER () FROM v",
            "WITH shared AS (SELECT id FROM t) SELECT * FROM shared", "BEGIN" })
            await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync(text, "reader"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("GRANT SELECT ON t TO reader"));
        await db.ExecuteAsync("BEGIN; GRANT SELECT ON v TO second");
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("SHOW VIEWS", "second"));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("SELECT * FROM v", "second"));
        await db.ExecuteAsync("ROLLBACK");
        Assert.Empty(await db.ListRoleViewsAsync("second"));
    }

    [Fact]
    public async Task Writes_use_view_names_visible_rows_base_defaults_and_constraints()
    {
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb, create: true);
        await db.ExecuteAsync("""
            CREATE TABLE docs(id INT PRIMARY KEY, qty INT CHECK(qty > 0), hidden BOOLEAN DEFAULT FALSE);
            INSERT INTO docs VALUES (1,10,FALSE),(2,20,TRUE);
            CREATE VIEW public_docs AS SELECT d.id AS number, d.qty AS amount FROM docs AS d WHERE d.hidden=FALSE;
            INSERT INTO public_docs(amount, number) VALUES (30,3);
            UPDATE public_docs SET amount=amount+1;
            DELETE FROM public_docs WHERE amount>=30;
            """);
        var rows = (await db.ExecuteAsync("SELECT id,qty FROM docs ORDER BY id")).Single().Rows;
        Assert.Equal(["1/11", "2/20"], rows.Select(r => string.Join("/", r.Select(v => v.Display))).ToArray());
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE public_docs SET hidden=TRUE"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM public_docs WHERE hidden=TRUE"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO public_docs VALUES(3,3),(1,1)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE public_docs SET amount=-1"));
        Assert.Equal("11", (await db.ExecuteAsync("SELECT amount FROM public_docs")).Single().Rows.Single()[0].Display);
        await db.ExecuteAsync("""
            CREATE VIEW joined AS SELECT a.id AS id FROM docs a JOIN docs b ON a.id=b.id;
            CREATE VIEW counts AS SELECT COUNT(*) AS n FROM docs;
            CREATE VIEW dedup AS SELECT DISTINCT qty FROM docs;
            """);
        foreach (var name in new[] { "joined", "counts", "dedup" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"DELETE FROM {name}"));
    }

    [Fact]
    public async Task Write_grants_are_independent_persistent_and_require_the_authenticated_write_API()
    {
        using (var create = await CatdbSqlConnection.OpenAsync(PathToDb, create: true))
            await create.ExecuteAsync("""
                CREATE TABLE t(id INT, hidden INT DEFAULT 10);
                CREATE VIEW v AS SELECT id FROM t;
                GRANT INSERT, UPDATE, DELETE ON v TO editor;
                """);
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb);
        await db.ExecuteRoleAsync("INSERT INTO v VALUES (1)", "EDITOR");
        await db.ExecuteRoleAsync("UPDATE v SET id=id+1", "editor");
        Assert.Equal("v", (await db.ExecuteRoleAsync("SHOW VIEWS", "editor")).Rows.Single()[0].Display);
        foreach (var sql in new[] { "SELECT * FROM v", "UPDATE t SET id=8", "UPDATE v SET id=hidden",
            "DELETE FROM v WHERE EXISTS (SELECT id FROM t)", "INSERT INTO v SELECT id FROM t",
            "GRANT SELECT ON v TO editor", "DELETE FROM v; DELETE FROM t" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync(sql, "editor"));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("DELETE FROM v", "editor"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("DELETE FROM v", "other"));
        await db.ExecuteAsync("BEGIN; REVOKE DELETE ON v FROM editor");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("UPDATE v SET id=3", "editor"));
        await db.ExecuteAsync("ROLLBACK");
        Assert.Equal("2", (await db.ExecuteAsync("SELECT id FROM t")).Single().Rows.Single()[0].Display);
        await db.ExecuteRoleAsync("DELETE FROM v", "editor");
        await db.ExecuteAsync("REVOKE INSERT, DELETE ON v FROM editor");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("INSERT INTO v VALUES(2)", "editor"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("DELETE FROM v", "editor"));
        await db.ExecuteAsync("DROP VIEW v; CREATE VIEW v AS SELECT * FROM t");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("UPDATE v SET hidden=1", "editor"));
    }

    [Fact]
    public async Task Base_table_grants_authorize_only_the_named_operation_on_that_table()
    {
        using (var create = await CatdbSqlConnection.OpenAsync(PathToDb, create: true))
            await create.ExecuteAsync("""
                CREATE TABLE inbox(id INT PRIMARY KEY, amount INT);
                CREATE TABLE secret(id INT);
                GRANT INSERT ON inbox TO operator;
                """);
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb);
        await db.ExecuteRoleAsync("INSERT INTO inbox VALUES (1,10)", "OPERATOR");
        foreach (var sql in new[] { "SELECT * FROM inbox", "INSERT INTO secret VALUES(2)",
            "UPDATE inbox SET amount=0", "DELETE FROM inbox", "INSERT INTO inbox SELECT id,id FROM secret" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync(sql, "operator"));
        await db.ExecuteAsync("GRANT UPDATE, DELETE ON inbox TO operator; REVOKE INSERT ON inbox FROM operator");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("INSERT INTO inbox VALUES(2,20)", "operator"));
        await db.ExecuteRoleAsync("UPDATE inbox SET amount=amount+2 WHERE id=1", "operator");
        Assert.Equal("12", (await db.ExecuteAsync("SELECT amount FROM inbox")).Single().Rows.Single()[0].Display);
        await db.ExecuteRoleAsync("DELETE FROM inbox WHERE id=1", "operator");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM inbox")).Single().Rows);
        await db.ExecuteAsync("DROP TABLE inbox; CREATE TABLE inbox(id INT)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("DELETE FROM inbox", "operator"));
    }

    public void Dispose() => Directory.Delete(directory, recursive: true);
}
