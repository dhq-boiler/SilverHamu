using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Nodes;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class BuiltInTypeTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Theory]
    [InlineData("UUID", "A0C2B845-CCAD-4289-90C7-504C001321FE", "a0c2b845-ccad-4289-90c7-504c001321fe")]
    [InlineData("DATE", "2024-02-29", "2024-02-29")]
    [InlineData("TIME", "23:59:59.9999999", "23:59:59.9999999")]
    [InlineData("LOCAL_DATETIME", "2024-02-29T10:30", "2024-02-29T10:30:00.0000000")]
    [InlineData("INSTANT", "2024-03-01T01:30:00+09:00", "2024-02-29T16:30:00.0000000Z")]
    [InlineData("DURATION", "-00:00:00.0000001", "-00:00:00.0000001")]
    public async Task Inserting_a_literal_and_casting_produce_equal_typed_values(string type, string input, string expected)
    {
        var db = Open();
        await db.ExecuteAsync($"CREATE TABLE samples(v {type}); INSERT INTO samples VALUES ('{input}')");
        var result = (await db.ExecuteAsync($"SELECT v FROM samples WHERE v = CAST('{expected}' AS {type})"))[0];
        var value = Assert.Single(Assert.Single(result.Rows));
        Assert.Equal(type, Assert.Single(result.ColumnTypes!));
        Assert.Equal(new SqlValue(type, expected), value);
    }

    [Theory]
    [InlineData("UUID", "12345678-zzzz-0000-0000-123456789012")]
    [InlineData("DATE", "2023-02-29")]
    [InlineData("TIME", "24:00:00")]
    [InlineData("LOCAL_DATETIME", "2024-01-01T12:00:00Z")]
    [InlineData("INSTANT", "2024-01-01T12:00:00")]
    [InlineData("INSTANT", "2024-01-01T12:00:00.00000001Z")]
    [InlineData("DURATION", "tomorrow")]
    public async Task Malformed_or_ambiguous_values_are_rejected_on_write_and_cast(string type, string input)
    {
        var db = Open();
        await db.ExecuteAsync($"CREATE TABLE samples(v {type})");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"INSERT INTO samples VALUES ('{input}')"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"SELECT CAST('{input}' AS {type})"));
        Assert.Empty((await db.ExecuteAsync("SELECT v FROM samples"))[0].Rows);
    }

    [Fact]
    public async Task Duration_index_ranges_order_negative_intervals_before_positive_intervals()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE spans(v DURATION);
            INSERT INTO spans VALUES ('00:00:01'),('-03:00:00'),('-01:00:00'),('00:00:00'),('-02:00:00');
            CREATE INDEX by_span ON spans(v)
            """);
        var values = (await db.ExecuteAsync("""
            SELECT v FROM spans
            WHERE v >= CAST('-02:00:00' AS DURATION) AND v < CAST('00:00:01' AS DURATION)
            ORDER BY v
            """))[0].Rows.Select(row => row[0].Text);
        Assert.Equal(new[] { "-02:00:00", "-01:00:00", "00:00:00" }, values);
    }

    [Fact]
    public async Task Decimal_enforces_scale_and_precision_without_rounding_a_literal()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE prices(v DECIMAL(3,2)); INSERT INTO prices VALUES (9.9900),(-9.99),(1.2)");
        foreach (var invalid in new[] { "10", "-10", "0.001", "0.00000000000000000000000000001" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"INSERT INTO prices VALUES ({invalid})"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT CAST('1.00000000000000000000000000001' AS DECIMAL(3,2))"));
        var result = (await db.ExecuteAsync("SELECT v FROM prices ORDER BY v"))[0];
        Assert.Equal("DECIMAL(3,2)", Assert.Single(result.ColumnTypes!));
        Assert.Equal(new[] { "-9.99", "1.20", "9.99" }, result.Rows.Select(row => row[0].Text));
        var wide = (await db.ExecuteAsync("SELECT CAST(9999999999999999999999999999 AS DECIMAL(28,0))"))[0];
        Assert.Equal("9999999999999999999999999999", Assert.Single(Assert.Single(wide.Rows)).Text);
    }

    [Fact]
    public async Task Numeric_rows_written_before_nominal_types_retain_real_semantics()
    {
        var store = new MemorySqlStore();
        var tableId = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes("legacy")));
        Task Put(string key, string json) => store.RequestAsync(new JsonObject
        {
            ["op"] = "put", ["key"] = SqlNames.Hex(key), ["value"] = SqlNames.Hex(json)
        });
        await store.RequestAsync(new JsonObject { ["op"] = "begin" });
        await Put("_catdb_owner/sql/v1/table/" + tableId, """
            {"Name":"legacy","Columns":[{"Name":"v","Type":"REAL","NotNull":false,"Unique":false,"PrimaryKey":false}]}
            """);
        await Put($"_catdb_owner/sql/v1/row/{tableId}/00000000000000000000000000000001", """{"v":{"Kind":"NUMBER","Text":"2.5"}}""");
        await store.RequestAsync(new JsonObject { ["op"] = "commit" });

        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("UPDATE legacy SET v=v+1");
        var result = (await db.ExecuteAsync("SELECT v,CAST(v AS DECIMAL) AS old_alias FROM legacy"))[0];
        Assert.Equal(new[] { "REAL", "REAL" }, result.ColumnTypes);
        Assert.All(Assert.Single(result.Rows), value => Assert.Equal(new SqlValue("NUMBER", "3.5"), value));
    }
}
