using System.Globalization;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class AdditionPrecisionTests
{
    private static async Task<SqlDatabase> Open(string baseType)
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync($"CREATE TYPE Amount AS DISTINCT {baseType}; ALTER TYPE Amount ENABLE OPERATOR +");
        return db;
    }

    private static string Add(string left, string right) =>
        $"SELECT CAST({left} AS Amount) + CAST({right} AS Amount)";

    [Theory]
    [InlineData("INTEGER", "9223372036854775806", "1", "9223372036854775807")]
    [InlineData("INTEGER", "-9223372036854775807", "-1", "-9223372036854775808")]
    [InlineData("DECIMAL(3,2)", "9.98", "0.01", "9.99")]
    [InlineData("REAL", "3961408125713216879677197516.7", "3961408125713216879677197516.8", "7922816251426433759354395033.5")]
    [InlineData("REAL", "7000000000000000000000000000.0", "-1.5", "6999999999999999999999999998.5")]
    public async Task Exact_sums_at_the_representation_boundary_retain_the_enabled_amount_type(
        string baseType, string left, string right, string expected)
    {
        var db = await Open(baseType);
        var result = (await db.ExecuteAsync(Add(left, right)))[0];
        Assert.Equal("Amount", Assert.Single(result.ColumnTypes!));
        var sum = Assert.Single(Assert.Single(result.Rows));
        Assert.Equal("Amount", sum.TypeName);
        Assert.Equal(decimal.Parse(expected, CultureInfo.InvariantCulture), decimal.Parse(sum.Text!, CultureInfo.InvariantCulture));
    }

    [Theory]
    [InlineData("INTEGER", "9223372036854775807", "1")]
    [InlineData("INTEGER", "-9223372036854775808", "-1")]
    [InlineData("DECIMAL(3,2)", "9.99", "0.01")]
    [InlineData("REAL", "79228162514264337593543950335", "1")]
    [InlineData("REAL", "10000000000000000000000000000", "0.1")]
    [InlineData("REAL", "3961408125713216879677197516.8", "3961408125713216879677197516.8")]
    public async Task Out_of_range_and_inexact_sums_cannot_be_stored(
        string baseType, string left, string right)
    {
        var db = await Open(baseType);
        await db.ExecuteAsync("CREATE TABLE totals(value Amount)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO totals " + Add(left, right)));
        Assert.Empty((await db.ExecuteAsync("SELECT value FROM totals"))[0].Rows);
    }
}
