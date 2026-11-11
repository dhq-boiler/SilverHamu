using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class UnicodeStringTests
{
    [Theory]
    [InlineData("U&'line1\\000Aline2'", "line1\nline2")]
    [InlineData("u&'\\000D\\000A\\0009'", "\r\n\t")]
    [InlineData("U&'back\\\\slash'", "back\\slash")]
    [InlineData("U&'O''Brien \\+01F600'", "O'Brien 😀")]
    [InlineData("'back\\slash'", "back\\slash")]
    [InlineData("N'back\\slash'", "back\\slash")]
    public void Unicode_escaped_strings_decode_controls_without_changing_plain_string_rules(string sql, string expected)
    {
        var select = Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse("SELECT " + sql)));
        Assert.Equal(expected, Assert.IsType<SqlLiteral>(select.Projections[0].Expression).Value.Text);
    }

    [Theory]
    [InlineData("U&'\\'")]
    [InlineData("U&'\\123'")]
    [InlineData("U&'\\GGGG'")]
    [InlineData("U&'\\+110000'")]
    [InlineData("U&'\\+00D800'")]
    public void Malformed_unicode_escapes_are_rejected(string sql) => Assert.Throws<SqlException>(() => SqlParser.Parse("SELECT " + sql));
}
