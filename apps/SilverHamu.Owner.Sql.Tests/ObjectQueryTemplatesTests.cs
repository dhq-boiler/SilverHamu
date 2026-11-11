using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ObjectQueryTemplatesTests
{
    private static readonly SqlTable Table = new("Book_3", [Column("ID", true), Column("Title")]);
    private static SqlColumn Column(string name, bool key = false) => new(name, "TEXT", false, false, key, null, null);

    [Fact]
    public void Tables_offer_both_selects_before_the_write_templates() =>
        Assert.Equal(new[] { "SELECT 1000行", "SELECT 全行", "SELECT 1000行して編集", "SELECT 全行して編集", "INSERT", "UPDATE", "DELETE" }, ObjectQueryTemplates.Create(Table).Select(t => t.Caption));

    [Theory]
    [InlineData("SelectEdit", 1000)]
    [InlineData("SelectAllEdit", null)]
    public void Edit_commands_are_read_only_selects_with_the_requested_limit(string operation, int? limit)
    {
        var template = ObjectQueryTemplates.Create(Table).Single(t => t.Operation == operation);
        Assert.True(template.EditResults);
        var query = Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse(template.Sql)));
        Assert.Equal(Table.Name, query.Table); Assert.Equal(limit, query.Limit);
    }

    [Theory]
    [InlineData("")]
    [InlineData("SELECT Title FROM Book_3")]
    public void Views_offer_both_selects_without_write_templates_even_when_columns_are_unknown(string query)
    {
        var templates = ObjectQueryTemplates.Create(new("public_books", [], ViewQuery: query));
        Assert.Equal(new[] { "Select", "SelectAll" }, templates.Select(t => t.Operation));
        Assert.All(templates, t => Assert.False(t.EditResults));
        foreach (var template in templates) Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse(template.Sql)));
    }

    [Theory]
    [InlineData("Book_3", "Book_3")]
    [InlineData("Order", "\"Order\"")]
    [InlineData("Book View", "\"Book View\"")]
    [InlineData("Odd\"Name", "\"Odd\"\"Name\"")]
    public void Select_templates_use_standard_row_limiting_and_safe_identifiers(string name, string expected)
    {
        var template = ObjectQueryTemplates.Create(Table with { Name = name })[0];
        Assert.Contains("FROM " + expected, template.Sql);
        Assert.Contains("FETCH FIRST 1000 ROWS ONLY", template.Sql);
        var select = Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse(template.Sql)));
        Assert.Equal(name, select.Table);
        Assert.Equal(1000, select.Limit);
    }

    [Theory]
    [InlineData("Book_3", "Book_3")]
    [InlineData("Order", "\"Order\"")]
    [InlineData("Book View", "\"Book View\"")]
    [InlineData("Odd\"Name", "\"Odd\"\"Name\"")]
    public void Select_all_has_no_row_limit_and_quotes_identifiers_safely(string name, string expected)
    {
        var template = ObjectQueryTemplates.Create(Table with { Name = name }).Single(t => t.Operation == "SelectAll");
        Assert.Equal($"SELECT *\nFROM {expected};", template.Sql);
        var select = Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse(template.Sql)));
        Assert.Equal(name, select.Table);
        Assert.Null(select.Limit);
    }

    [Fact]
    public async Task Select_all_returns_rows_beyond_the_first_thousand()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE books (id INT); INSERT INTO books VALUES " + string.Join(",", Enumerable.Range(1, 1001).Select(i => $"({i})")));
        var templates = ObjectQueryTemplates.Create(new("books", []));
        Assert.Equal(1000, Assert.Single(await db.ExecuteAsync(templates.Single(t => t.Operation == "Select").Sql)).Rows.Count);
        Assert.Equal(1001, Assert.Single(await db.ExecuteAsync(templates.Single(t => t.Operation == "SelectAll").Sql)).Rows.Count);
    }

    [Theory]
    [InlineData(false, false)]
    [InlineData(true, false)]
    [InlineData(false, true)]
    public async Task Every_unedited_write_template_is_a_syntax_error_before_any_store_request(bool withoutKeys, bool withoutColumns)
    {
        var table = withoutColumns ? new SqlTable("Book_3", []) : withoutKeys ? Table with { Columns = [Column("Title")] } : Table;
        var store = new MemorySqlStore();
        int requests = 0;
        var db = new SqlDatabase(request => { requests++; return store.RequestAsync(request); }, () => store.InTransaction);
        foreach (var template in ObjectQueryTemplates.Create(table).Where(t => t.Operation is "Insert" or "Update" or "Delete"))
        {
            Assert.Contains("{{", template.Sql);
            Assert.Throws<SqlException>(() => SqlParser.Parse(template.Sql));
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM Book_3;\n" + template.Sql));
            Assert.Equal(0, requests);
            Assert.False(store.InTransaction);
        }
    }

    [Fact]
    public void Update_uses_non_key_columns_and_targets_every_component_of_a_composite_key()
    {
        var table = new SqlTable("books", [Column("tenant"), Column("ID"), Column("Title")], CompositePrimaryKey: ["tenant", "ID"]);
        var update = ObjectQueryTemplates.Create(table).Single(t => t.Operation == "Update").Sql;
        Assert.Contains("Title = {{値_1}}", update);
        Assert.Contains("tenant = {{キー_1}}", update);
        Assert.Contains("AND ID = {{キー_2}}", update);
        Assert.DoesNotContain("SET\n    tenant", update);
    }

    [Fact]
    public void Tables_without_keys_require_an_explicit_where_predicate()
    {
        var table = new SqlTable("books", [Column("Title")]);
        foreach (var template in ObjectQueryTemplates.Create(table).Where(t => t.Operation is "Update" or "Delete"))
            Assert.Contains("WHERE\n    {{WHERE条件を入力}}", template.Sql);
    }

    [Fact]
    public void Insert_lists_all_columns_with_one_required_value_per_column()
    {
        var sql = ObjectQueryTemplates.Create(Table).Single(t => t.Operation == "Insert").Sql;
        Assert.Contains("ID,\n    Title", sql);
        Assert.Contains("{{値_1}},\n    {{値_2}}", sql);
    }

    [Theory]
    [InlineData("\n")]
    [InlineData("\r")]
    [InlineData("\r\n")]
    public void Editing_required_placeholders_produces_parseable_insert_update_and_delete_statements(string newline)
    {
        foreach (var template in ObjectQueryTemplates.Create(Table).Where(t => t.Operation is "Insert" or "Update" or "Delete"))
        {
            var edited = template.Sql.Replace("{{値_1}}", "1").Replace("{{値_2}}", "'title'").Replace("{{キー_1}}", "1")
                .ReplaceLineEndings(newline);
            Assert.Single(SqlParser.Parse(edited));
        }
    }
}
