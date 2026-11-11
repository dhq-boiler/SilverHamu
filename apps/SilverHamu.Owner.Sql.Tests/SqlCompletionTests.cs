using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class SqlCompletionTests
{
    private static readonly SqlTable[] Schema =
    [
        new("Book_3", [Column("ID"), Column("PublishDate"), Column("Title")]),
        new("Authors", [Column("AuthorName")]),
        new("Book View", [Column("Odd\"Name")], ViewQuery: "SELECT Title FROM Book_3")
    ];
    private static SqlColumn Column(string name) => new(name, "TEXT", false, false, false, null, null);

    [Theory]
    [InlineData("SELECT * FROM Book_3 b WHERE ")]
    [InlineData("SELECT * FROM Book_3 AS b WHERE ")]
    [InlineData("SELECT * FROM Book_3 b WHERE ID=1 AND ")]
    public void Table_aliases_are_prioritized_as_variables_at_an_empty_predicate(string sql)
    {
        var set = SqlCompletion.Suggest(sql, sql.Length, Schema);
        var alias = Assert.Single(set.Items, i => i.Name == "b" && i.Kind == "変数");
        Assert.Equal(alias, set.Items[0]);
        Assert.Equal("b  · 変数", alias.Label);
        Assert.Equal((sql + "b", sql.Length + 1), set.Apply(sql, alias));
        var columns = SqlCompletion.Suggest(sql + "b.", sql.Length + 2, Schema);
        Assert.Equal(new[] { "ID", "PublishDate", "Title" }, columns.Items.Select(i => i.Name));
        Assert.All(columns.Items, i => Assert.Equal("列", i.Kind));
    }

    [Fact]
    public void Alias_candidates_follow_the_statement_and_do_not_require_loaded_schema()
    {
        const string sql = "SELECT * FROM Authors old; SELECT * FROM Book_3 b JOIN Authors a ON ";
        var set = SqlCompletion.Suggest(sql, sql.Length, []);
        Assert.Equal(new[] { "a", "b" }, set.Items.Where(i => i.Kind == "変数").Select(i => i.Name));
        Assert.DoesNotContain(set.Items, i => i.Name == "old");
        const string tablePosition = "SELECT * FROM Book_3 b JOIN ";
        Assert.DoesNotContain(SqlCompletion.Suggest(tablePosition, tablePosition.Length, Schema).Items, i => i.Kind == "変数");
    }

    [Fact]
    public void Quoted_aliases_keep_their_quotes_when_inserted()
    {
        const string sql = "SELECT * FROM Book_3 AS \"book alias\" WHERE ";
        var set = SqlCompletion.Suggest(sql, sql.Length, Schema);
        var alias = Assert.Single(set.Items, i => i.Kind == "変数");
        Assert.Equal("book alias", alias.Name);
        Assert.EndsWith("WHERE \"book alias\"", set.Apply(sql, alias).Text);
    }

    [Fact]
    public void Keywords_ignore_case_and_replace_the_whole_word_at_the_caret()
    {
        const string sql = "selECT * FROM Book_3;";
        var set = SqlCompletion.Suggest(sql, 3, Schema);
        var item = Assert.Single(set.Items, i => i.Name == "SELECT");
        Assert.Equal(("SELECT * FROM Book_3;", 6), set.Apply(sql, item));
        var atStart = SqlCompletion.Suggest(sql, 0, Schema, force: true);
        Assert.Equal(("SELECT * FROM Book_3;", 6), atStart.Apply(sql, Assert.Single(atStart.Items, i => i.Name == "SELECT")));
    }

    [Theory]
    [InlineData("SELECT * FROM Bo")]
    [InlineData("SELECT * FROM [Bo")]
    [InlineData("SELECT * FROM \"Bo")]
    [InlineData("SELECT * FROM Book_3 JOIN Bo")]
    public void Table_positions_offer_tables_and_views_with_standard_quoted_identifiers(string sql)
    {
        var set = SqlCompletion.Suggest(sql, sql.Length, Schema);
        Assert.Equal(2, set.Items.Count);
        Assert.All(set.Items, i => Assert.Contains(i.Kind, new[] { "テーブル", "ビュー" }));
        var view = Assert.Single(set.Items, i => i.Name == "Book View");
        Assert.EndsWith("\"Book View\"", set.Apply(sql, view).Text);
        var completed = set.Apply(sql, view).Text;
        Assert.NotEmpty(SqlParser.Parse(sql.Contains("JOIN") ? completed + " ON 1=1" : completed));
    }

    [Theory]
    [InlineData("SELECT b.Pu FROM Book_3 AS b", "Pu", "PublishDate")]
    [InlineData("SELECT Book_3.Ti FROM Book_3", "Ti", "Title")]
    [InlineData("SELECT b.Ti FROM \"Book_3\" b", "Ti", "Title")]
    public void Qualified_columns_resolve_table_names_and_aliases_even_before_from(string sql, string prefix, string expected)
    {
        int cursor = sql.IndexOf(prefix, StringComparison.Ordinal) + prefix.Length;
        var set = SqlCompletion.Suggest(sql, cursor, Schema);
        var item = Assert.Single(set.Items);
        Assert.Equal(expected, item.Name);
        Assert.Contains("." + expected, set.Apply(sql, item).Text);
    }

    [Fact]
    public void Unqualified_columns_use_the_current_statement_sources()
    {
        const string sql = "SELECT Book_3.ID FROM Book_3; SELECT Au FROM Book_3";
        int cursor = sql.LastIndexOf("Au", StringComparison.Ordinal) + 2;
        Assert.DoesNotContain(SqlCompletion.Suggest(sql, cursor, Schema).Items, i => i.Name == "AuthorName");
        const string next = "SELECT Book_3.ID FROM Book_3; SELECT Au FROM Authors";
        var set = SqlCompletion.Suggest(next, next.LastIndexOf("Au", StringComparison.Ordinal) - " FROM ".Length, Schema);
        Assert.Contains(set.Items, i => i.Name == "AuthorName");
    }

    [Theory]
    [InlineData("SELECT 'Pub")]
    [InlineData("SELECT '")]
    [InlineData("SELECT 'P'")]
    [InlineData("SELECT 'it''s Pub")]
    [InlineData("SELECT -- Pub")]
    [InlineData("SELECT /* Pub")]
    [InlineData("SELECT --")]
    [InlineData("SELECT /*")]
    public void Strings_and_comments_never_offer_completions_even_when_explicitly_requested(string sql) =>
        Assert.Empty(SqlCompletion.Suggest(sql, sql.Length, Schema, force: true).Items);

    [Theory]
    [InlineData("\n")]
    [InlineData("\r")]
    [InlineData("\r\n")]
    public void Completion_resumes_after_a_line_comment_with_any_editor_line_ending(string newline)
    {
        var sql = "-- comment" + newline + "SELECT * FROM Book_3 WHERE Pu";
        Assert.Equal("PublishDate", Assert.Single(SqlCompletion.Suggest(sql, sql.Length, Schema).Items).Name);
    }

    [Fact]
    public void Empty_prefix_requires_a_request_except_after_table_clauses_and_dots()
    {
        Assert.Empty(SqlCompletion.Suggest("", 0, Schema).Items);
        Assert.Contains(SqlCompletion.Suggest("", 0, Schema, force: true).Items, i => i.Name == "SELECT");
        Assert.NotEmpty(SqlCompletion.Suggest("SELECT * FROM ", 14, Schema).Items);
        const string sql = "SELECT b. FROM Book_3 b";
        Assert.Equal(3, SqlCompletion.Suggest(sql, 9, Schema).Items.Count);
        Assert.Empty(SqlCompletion.Suggest("SELECT", 6, Schema).Items);
        Assert.Contains(SqlCompletion.Suggest("SELECT", 6, Schema, force: true).Items, i => i.Name == "SELECT");
    }

    [Fact]
    public void Schema_changes_replace_old_candidates_and_duplicate_column_names_are_not_repeated()
    {
        var first = SqlCompletion.Suggest("Pu", 2, Schema);
        Assert.Contains(first.Items, i => i.Name == "PublishDate");
        Assert.Empty(SqlCompletion.Suggest("Pu", 2, []).Items);
        var duplicate = SqlCompletion.Suggest("Pu", 2, Schema.Concat(Schema));
        Assert.Single(duplicate.Items, i => i.Name == "PublishDate");
    }

    [Fact]
    public void Insertion_escapes_identifier_quotes_and_preserves_surrounding_text()
    {
        const string sql = "SELECT Odd FROM \"Book View\";";
        var set = SqlCompletion.Suggest(sql, 10, Schema);
        var item = Assert.Single(set.Items);
        var inserted = set.Apply(sql, item);
        Assert.Equal("SELECT \"Odd\"\"Name\" FROM \"Book View\";", inserted.Text);
        Assert.Equal(18, inserted.Cursor);
        Assert.Single(SqlParser.Parse(inserted.Text));
    }
}
