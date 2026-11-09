using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class ProcedureTests : IDisposable
{
    private readonly string directory = Directory.CreateTempSubdirectory("catdb-procedure-").FullName;
    private string PathToDb => Path.Combine(directory, "procedures.ndb");

    [Fact]
    public async Task Committed_procedures_reopen_and_execute_as_one_unit()
    {
        using (var setup = await CatdbSqlConnection.OpenAsync(PathToDb, create: true))
        {
            await setup.ExecuteAsync("""
                CREATE TABLE ledger(id INT PRIMARY KEY, memo TEXT);
                CREATE PROCEDURE [post item] AS BEGIN
                    INSERT INTO ledger VALUES(1,'BEGIN; END');
                    UPDATE ledger SET memo='done' WHERE id=1;
                    SELECT memo FROM ledger
                END;
                CREATE PROCEDURE duplicate AS BEGIN
                    INSERT INTO ledger VALUES(2,'part one');
                    INSERT INTO ledger VALUES(1,'duplicate key');
                END;
                """);
        }
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb);
        Assert.Equal("done", Assert.Single(Assert.Single((await db.ExecuteAsync("EXECUTE [POST ITEM]"))[0].Rows)).Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXEC duplicate"));
        Assert.Empty((await db.ExecuteAsync("SELECT id FROM ledger WHERE id=2"))[0].Rows);
        await db.ExecuteAsync("BEGIN; INSERT INTO ledger VALUES(9,'previous')");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXEC duplicate"));
        Assert.False(db.InTransaction);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM ledger"))[0].Rows);
        await db.ExecuteAsync("BEGIN; DROP PROCEDURE duplicate; ROLLBACK; DROP PROCEDURE duplicate");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXEC duplicate"));
    }

    [Theory]
    [InlineData("EXEC p")]
    [InlineData("CREATE PROCEDURE nested AS BEGIN SELECT 1 END")]
    [InlineData("ROLLBACK")]
    [InlineData("DROP TABLE t")]
    [InlineData("GRANT SELECT ON v TO r")]
    public void Procedures_reject_nested_calls_transaction_control_and_ddl(string body)
    {
        Assert.Throws<SqlException>(() => SqlParser.Parse($"CREATE PROCEDURE p AS BEGIN {body}; END"));
    }

    [Fact]
    public async Task Explicit_transaction_owns_procedure_writes_and_role_cannot_call_it()
    {
        using var db = await CatdbSqlConnection.OpenAsync(PathToDb, create: true);
        await db.ExecuteAsync("CREATE TABLE t(id INT); CREATE VIEW v AS SELECT id FROM t; GRANT SELECT ON v TO r; CREATE PROCEDURE p AS BEGIN INSERT INTO t VALUES(1) END");
        await db.ExecuteAsync("BEGIN; EXEC p");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t"))[0].Rows);
        await db.ExecuteAsync("ROLLBACK");
        await Assert.ThrowsAsync<SqlException>(() => db.QueryRoleViewAsync("EXEC p", "r"));
        Assert.Empty((await db.QueryRoleViewAsync("SELECT * FROM v", "r")).Rows);
    }

    public void Dispose() => Directory.Delete(directory, recursive: true);
}
