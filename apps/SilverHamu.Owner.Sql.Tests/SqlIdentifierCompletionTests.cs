using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class SqlIdentifierCompletionTests
{
    [Theory]
    [InlineData("Book_3", "Book_3")]
    [InlineData("PublishDate", "PublishDate")]
    [InlineData("_books", "_books")]
    [InlineData("書籍", "書籍")]
    [InlineData("SELECT", "\"SELECT\"")]
    [InlineData("select", "\"select\"")]
    [InlineData("Order", "\"Order\"")]
    [InlineData("CURRENT_USER", "\"CURRENT_USER\"")]
    [InlineData("CURRENT_DEFAULT_TRANSFORM_GROUP", "\"CURRENT_DEFAULT_TRANSFORM_GROUP\"")]
    [InlineData("JSON_TABLE", "\"JSON_TABLE\"")]
    [InlineData("TOP", "\"TOP\"")]
    [InlineData("Book View", "\"Book View\"")]
    [InlineData("1Book", "\"1Book\"")]
    [InlineData("book-title", "\"book-title\"")]
    [InlineData("book.title", "\"book.title\"")]
    [InlineData("book$title", "\"book$title\"")]
    [InlineData("Odd\"Name", "\"Odd\"\"Name\"")]
    public void Table_and_column_candidates_quote_only_names_that_need_delimiters(string name, string expected)
    {
        SqlTable[] schema = [new(name, []), new("books", [new(name, "TEXT", false, false, false, null, null)])];
        const string from = "SELECT * FROM ";
        var tableSet = SqlCompletion.Suggest(from, from.Length, schema, force: true);
        var table = Assert.Single(tableSet.Items, i => i.Name == name && i.Kind == "テーブル");
        Assert.Equal(expected, table.InsertText);
        Assert.Single(SqlParser.Parse(tableSet.Apply(from, table).Text));

        const string select = "SELECT books.";
        var columnSet = SqlCompletion.Suggest(select + " FROM books", select.Length, schema, force: true);
        var column = Assert.Single(columnSet.Items, i => i.Name == name && i.Kind == "列");
        Assert.Equal(expected, column.InsertText);
        Assert.Single(SqlParser.Parse(columnSet.Apply(select + " FROM books", column).Text));
    }

    [Theory]
    [InlineData("SELECT * FROM \"Bo\"", 17)]
    [InlineData("SELECT * FROM [Bo]", 16)]
    public void Completing_inside_existing_quotes_replaces_both_delimiters_for_regular_names(string sql, int cursor)
    {
        var set = SqlCompletion.Suggest(sql, cursor, [new("Book_3", [])]);
        var item = Assert.Single(set.Items);
        Assert.Equal("SELECT * FROM Book_3", set.Apply(sql, item).Text);
    }

    [Theory]
    [InlineData("UPDATE books SET ", "PublishDate", "PublishDate", "=NULL")]
    [InlineData("UPDATE books SET ", "Order", "\"Order\"", "=NULL")]
    [InlineData("DELETE FROM books WHERE ", "PublishDate", "PublishDate", " IS NULL")]
    [InlineData("DELETE FROM books WHERE ", "Order", "\"Order\"", " IS NULL")]
    [InlineData("INSERT INTO books (", "PublishDate", "PublishDate", ") VALUES (NULL)")]
    [InlineData("INSERT INTO books (", "Order", "\"Order\"", ") VALUES (NULL)")]
    public void Write_statements_apply_the_same_identifier_rule(string sql, string name, string expected, string suffix)
    {
        SqlTable[] schema = [new("books", [new(name, "TEXT", false, false, false, null, null)])];
        var set = SqlCompletion.Suggest(sql, sql.Length, schema);
        var item = Assert.Single(set.Items, i => i.Kind == "列");
        Assert.Equal(expected, item.InsertText);
        Assert.Single(SqlParser.Parse(set.Apply(sql, item).Text + suffix));
    }
}
