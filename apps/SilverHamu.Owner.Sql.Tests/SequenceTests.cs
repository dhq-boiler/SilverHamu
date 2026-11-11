using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class SequenceTests
{
    [Fact]
    public async Task Allocations_follow_rows_transactions_and_savepoints_and_survive_reopening()
    {
        var directory = Directory.CreateTempSubdirectory("silverhamu-sequences-");
        var path = Path.Combine(directory.FullName, "sequences.ndb");
        try
        {
            using (var db = await SilverHamuSqlConnection.OpenAsync(path, true))
            {
                await db.ExecuteAsync("""
                    CREATE SEQUENCE ids START WITH -2 INCREMENT BY 2;
                    CREATE TABLE t(id INT PRIMARY KEY, value TEXT UNIQUE);
                    INSERT INTO t VALUES(NEXT VALUE FOR IDS,'a'),(NEXT VALUE FOR ids,'b');
                    BEGIN; SAVEPOINT s; SELECT NEXT VALUE FOR ids; ROLLBACK TO s
                    """);
                await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES(NEXT VALUE FOR ids,'a')"));
                var next = Assert.Single(await db.ExecuteAsync("SELECT NEXT VALUE FOR ids AS ticket"));
                Assert.Equal("ticket", Assert.Single(next.Columns));
                Assert.Equal("2", Assert.Single(Assert.Single(next.Rows)).Text);
                await db.ExecuteAsync("COMMIT; BEGIN; DROP SEQUENCE ids; ROLLBACK");
            }
            using var reopened = await SilverHamuSqlConnection.OpenAsync(path);
            Assert.Equal("4", Assert.Single((await reopened.ExecuteAsync("SELECT NEXT VALUE FOR ids"))[0].Rows)[0].Text);
            Assert.Equal(new[] { "-2","0" }, (await reopened.ExecuteAsync("SELECT id FROM t ORDER BY id"))[0].Rows.Select(r => r[0].Text));
        }
        finally { directory.Delete(recursive: true); }
    }

    [Theory]
    [InlineData("9223372036854775807", "1")]
    [InlineData("-9223372036854775808", "-1")]
    public async Task The_final_bigint_is_available_once_without_wraparound(string limit, string increment)
    {
        var store = new SilverHamu.Owner.Services.MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync($"CREATE SEQUENCE edge START WITH {limit} INCREMENT BY {increment}");
        Assert.Equal(limit, Assert.Single((await db.ExecuteAsync("SELECT NEXT VALUE FOR edge"))[0].Rows)[0].Text);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT NEXT VALUE FOR edge"));
        await db.ExecuteAsync("DROP SEQUENCE edge");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT NEXT VALUE FOR edge"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE SEQUENCE bad INCREMENT BY 0"));
    }
}
