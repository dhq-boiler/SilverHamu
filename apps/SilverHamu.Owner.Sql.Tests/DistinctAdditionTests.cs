using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class DistinctAdditionTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Existing_columns_add_in_select_and_preserve_the_currency_through_a_view()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TYPE JpyAmount AS DISTINCT DECIMAL(18,0);
            CREATE TABLE 明細 (本体 JpyAmount, 税額 JpyAmount);
            INSERT INTO 明細 VALUES (1000,100),(400,NULL)
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT 本体 + 税額 FROM 明細"));
        await db.ExecuteAsync("""
            ALTER TYPE JpyAmount ENABLE OPERATOR +;
            CREATE VIEW 合計表 AS SELECT 本体＋税額 AS [税込＋金額] FROM 明細;
            GRANT SELECT ON 合計表 TO reader
            """);
        var result = await db.QueryRoleViewAsync("SELECT [税込＋金額] FROM 合計表 ORDER BY [税込＋金額]", "reader");
        Assert.Equal("JpyAmount", Assert.Single(result.ColumnTypes!));
        Assert.Equal(new string?[] { null, "1100" }, result.Rows.Select(row => row[0].Text));
        Assert.Equal("JpyAmount", result.Rows[1][0].TypeName);
        var empty = Assert.Single(await db.ExecuteAsync("SELECT 本体 + 税額 FROM 明細 WHERE 本体 IS NULL"));
        Assert.Empty(empty.Rows);
        Assert.Equal("JpyAmount", Assert.Single(empty.ColumnTypes!));
    }

    [Fact]
    public async Task Both_operands_must_have_the_same_addition_enabled_type_even_when_the_table_is_empty()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TYPE Yen AS DISTINCT INTEGER;
            CREATE TYPE Dollar AS DISTINCT INTEGER;
            CREATE TYPE ItemId AS DISTINCT INTEGER;
            ALTER TYPE Yen ENABLE OPERATOR +;
            ALTER TYPE Dollar ENABLE OPERATOR +;
            CREATE TABLE t (j Yen, d Dollar, id ItemId)
            """);
        foreach (var expression in new[] { "j + d", "d + j", "j + 1", "1 + j", "id + id", "j - j" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"SELECT {expression} FROM t"));
        var result = Assert.Single(await db.ExecuteAsync("SELECT NULL + j, d + NULL FROM t"));
        Assert.Equal(new[] { "Yen", "Dollar" }, result.ColumnTypes);
        Assert.Empty(result.Rows);
    }

    [Fact]
    public async Task Every_sum_checks_the_return_type_before_an_update_can_change_rows()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TYPE Credit AS DISTINCT DECIMAL(5,2) CHECK (VALUE <= 100);
            ALTER TYPE Credit ENABLE OPERATOR +;
            CREATE TABLE credits(id INT PRIMARY KEY, balance Credit, bonus Credit);
            INSERT INTO credits VALUES (1,10.01,0.02),(2,90.00,10.01)
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE credits SET balance = balance + bonus"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT balance + bonus FROM credits WHERE id = 2"));
        var balances = Assert.Single(await db.ExecuteAsync("SELECT balance FROM credits ORDER BY id"));
        Assert.Equal(new[] { "10.01", "90.00" }, balances.Rows.Select(row => row[0].Text));
        var total = Assert.Single(await db.ExecuteAsync("""
            WITH valid AS (SELECT balance + bonus AS subtotal FROM credits WHERE id = 1)
            SELECT subtotal + CAST(0.01 AS Credit) AS total FROM valid
            """));
        var value = Assert.Single(Assert.Single(total.Rows));
        Assert.Equal("10.04", value.Text);
        Assert.Equal("Credit", value.TypeName);
    }
}
