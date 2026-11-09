using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class OperatorCatalogTests
{
    private const string Enable = "ALTER TYPE Amount ENABLE OPERATOR +";

    [Theory]
    [InlineData("INTEGER")]
    [InlineData("REAL")]
    [InlineData("DECIMAL(18,2)")]
    public async Task A_numeric_type_can_enable_addition_after_columns_already_use_it(string baseType)
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync($"CREATE TYPE Amount AS DISTINCT {baseType}; CREATE TABLE prices(value Amount); INSERT INTO prices VALUES(2)");
        await db.ExecuteAsync(Enable);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TYPE AMOUNT ENABLE OPERATOR +"));
        var result = Assert.Single(await db.ExecuteAsync("SELECT value + value FROM prices"));
        Assert.Equal("Amount", Assert.Single(result.ColumnTypes!));
    }

    [Fact]
    public async Task Invalid_statements_do_not_enable_addition()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TYPE Amount AS DISTINCT DECIMAL(9,2); CREATE TYPE Tag AS DISTINCT TEXT");
        foreach (var sql in new[]
        {
            "CREATE OPERATOR + (Amount, Amount) RETURNS Amount",
            "ALTER TYPE Amount ENABLE OPERATOR + RETURNS Other",
            "ALTER TYPE Tag ENABLE OPERATOR +",
            "ALTER TYPE Unknown ENABLE OPERATOR +",
            "ALTER TYPE INTEGER ENABLE OPERATOR +",
            "ALTER TYPE Amount ENABLE OPERATOR -"
        })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(sql));
        await db.ExecuteAsync(Enable);
    }

    [Fact]
    public async Task Only_committed_addition_settings_survive_reopen()
    {
        var directory = Directory.CreateTempSubdirectory("catdb-addition-");
        var path = Path.Combine(directory.FullName, "catalog.ndb");
        try
        {
            using (var db = await CatdbSqlConnection.OpenAsync(path, create: true))
            {
                await db.ExecuteAsync($"CREATE TYPE Amount AS DISTINCT INTEGER; BEGIN; {Enable}; ROLLBACK");
                await db.ExecuteAsync($"BEGIN; SAVEPOINT mark; {Enable}; ROLLBACK TO mark; {Enable}; COMMIT");
            }
            using var reopened = await CatdbSqlConnection.OpenAsync(path);
            await Assert.ThrowsAsync<SqlException>(() => reopened.ExecuteAsync(Enable));
            var result = Assert.Single(await reopened.ExecuteAsync("SELECT CAST(1 AS Amount) + CAST(2 AS Amount)"));
            var sum = Assert.Single(Assert.Single(result.Rows));
            Assert.Equal("3", sum.Text);
            Assert.Equal("Amount", sum.TypeName);
        }
        finally { directory.Delete(recursive: true); }
    }
}
