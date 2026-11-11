using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class SqlWriteCompletionTests
{
    private static readonly SqlTable[] Schema =
    [
        new("Book_3", [Column("ID"), Column("Title"), Column("PublishDate")]),
        new("Authors", [Column("AuthorName")])
    ];
    private static SqlColumn Column(string name) => new(name, "TEXT", false, false, false, null, null);
    private static SqlCompletionSet Suggest(string sql) => SqlCompletion.Suggest(sql, sql.Length, Schema);

    [Theory]
    [InlineData("UPDATE ")]
    [InlineData("DELETE FROM ")]
    [InlineData("INSERT INTO ")]
    public void Write_targets_offer_only_tables_and_views(string sql)
    {
        Assert.Equal(new[] { "Authors", "Book_3" }, Suggest(sql).Items.Select(i => i.Name));
        Assert.All(Suggest(sql).Items, i => Assert.Equal("テーブル", i.Kind));
    }

    [Theory]
    [InlineData("DELETE ", "FROM")]
    [InlineData("INSERT ", "INTO")]
    [InlineData("UPDATE Book_3 ", "SET")]
    [InlineData("DELETE FROM Book_3 ", "WHERE")]
    [InlineData("UPDATE Book_3 SET Title ", "=")]
    public void Required_clauses_offer_the_next_supported_token(string sql, string expected) =>
        Assert.Equal(expected, Assert.Single(Suggest(sql).Items).Name);

    [Theory]
    [InlineData("UPDATE Book_3 SET ")]
    [InlineData("UPDATE \"Book_3\" SET ")]
    [InlineData("UPDATE /* target */ Book_3 SET ")]
    [InlineData("INSERT INTO Book_3 (")]
    public void Column_positions_offer_only_the_target_table_columns(string sql)
    {
        var set = Suggest(sql);
        Assert.Equal(new[] { "ID", "PublishDate", "Title" }, set.Items.Select(i => i.Name));
        Assert.All(set.Items, i => Assert.Equal("列", i.Kind));
    }

    [Theory]
    [InlineData("UPDATE Book_3 SET Title='abc', ")]
    [InlineData("UPDATE Book_3 SET Title=COALESCE(NULL,'a,b'), ")]
    [InlineData("UPDATE Book_3 SET Title='-- a,b', ")]
    [InlineData("INSERT INTO Book_3 (Title, ")]
    public void Completed_assignments_and_insert_columns_are_not_suggested_again(string sql)
    {
        Assert.Equal(new[] { "ID", "PublishDate" }, Suggest(sql).Items.Select(i => i.Name));
    }

    [Theory]
    [InlineData("UPDATE Book_3 SET Title=COALESCE(NULL, ")]
    [InlineData("UPDATE Book_3 SET Title= ")]
    [InlineData("UPDATE Book_3 SET Title='WHERE', PublishDate= ")]
    [InlineData("DELETE FROM Book_3 WHERE ")]
    [InlineData("UPDATE Book_3 SET Title='abc' WHERE ")]
    public void Expressions_and_predicates_use_target_columns_and_value_keywords(string sql)
    {
        var set = Suggest(sql);
        Assert.Contains(set.Items, i => i.Name == "Title" && i.Kind == "列");
        Assert.Contains(set.Items, i => i.Name == "NULL");
        Assert.DoesNotContain(set.Items, i => i.Name == "AuthorName" || i.Kind is "テーブル" or "変数");
    }

    [Theory]
    [InlineData("INSERT INTO Book_3 VALUES (")]
    [InlineData("INSERT INTO Book_3 (ID,Title) VALUES (1, ")]
    public void Insert_values_offer_literal_forms_instead_of_column_references(string sql)
    {
        var set = Suggest(sql);
        Assert.Contains(set.Items, i => i.Name == "NULL");
        Assert.Contains(set.Items, i => i.Name == "TRUE");
        Assert.Contains(set.Items, i => i.Name == "CAST");
        Assert.All(set.Items, i => Assert.Equal("SQL", i.Kind));
        Assert.DoesNotContain(set.Items, i => i.Name == "SELECT" || i.Name == "WHERE");
    }

    [Fact]
    public void Insert_source_queries_use_the_source_schema_instead_of_destination_columns()
    {
        const string sql = "INSERT INTO Book_3 (Title) SELECT Ti FROM Authors";
        int cursor = sql.IndexOf("SELECT Ti", StringComparison.Ordinal) + "SELECT Ti".Length;
        Assert.DoesNotContain(SqlCompletion.Suggest(sql, cursor, Schema).Items, i => i.Kind == "列");
        const string valid = "INSERT INTO Book_3 (Title) SELECT AuthorN FROM Authors";
        cursor = valid.IndexOf("SELECT AuthorN", StringComparison.Ordinal) + "SELECT AuthorN".Length;
        Assert.Equal("AuthorName", Assert.Single(SqlCompletion.Suggest(valid, cursor, Schema).Items).Name);
    }

    [Fact]
    public void Insert_column_lists_transition_to_supported_row_sources()
    {
        Assert.Equal(new[] { "SELECT", "WITH", "VALUES" }, Suggest("INSERT INTO Book_3 (Title) ").Items.Select(i => i.Name));
        Assert.Equal("(", Assert.Single(Suggest("INSERT INTO Book_3 VALUES ").Items).Name);
        Assert.Equal("(", Assert.Single(Suggest("INSERT INTO Book_3 VALUES ('x'), ").Items).Name);
    }

    [Theory]
    [InlineData("INSERT INTO Book_3 VALUES (1 ")]
    [InlineData("INSERT INTO Book_3 VALUES ('x' ")]
    [InlineData("INSERT INTO Book_3 VALUES (NULL ")]
    [InlineData("INSERT INTO Book_3 VALUES (CAST(NULL AS TEXT) ")]
    public void Completed_values_offer_separators_instead_of_another_literal(string sql) =>
        Assert.Equal(new[] { ")", "," }, Suggest(sql).Items.Select(i => i.Name));

    [Theory]
    [InlineData("UPDATE Book_3 SET Ti", "Title", "='x'")]
    [InlineData("DELETE FROM Book_3 WHERE Pu", "PublishDate", " IS NULL")]
    [InlineData("INSERT INTO Book_3 (Ti", "Title", ") VALUES ('x')")]
    public void Inserted_columns_form_valid_write_statements(string sql, string column, string suffix)
    {
        var set = Suggest(sql);
        var item = Assert.Single(set.Items, i => i.Name == column);
        var completed = set.Apply(sql, item).Text;
        Assert.Equal(column, item.InsertText);
        Assert.Contains(column, completed);
        Assert.Single(SqlParser.Parse(completed + suffix));
    }

    [Fact]
    public void Earlier_statements_and_closed_comments_do_not_change_write_context()
    {
        Assert.Equal("AuthorName", Assert.Single(Suggest("UPDATE Book_3 SET Title='x'; DELETE FROM Authors WHERE AuthorN").Items).Name);
        Assert.Equal("SET", Assert.Single(Suggest("UPDATE Book_3 /* WHERE Title=1 */ ").Items).Name);
        Assert.Empty(Suggest("INSERT INTO Book_3 VALUES ('Ti").Items);
    }
}
