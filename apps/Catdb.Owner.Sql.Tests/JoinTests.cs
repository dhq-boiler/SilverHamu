using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class JoinTests
{
    private static SqlDatabase Database()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Inner_join_preserves_duplicate_matches_and_excludes_null_and_unmatched_keys()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE books (id INT, author_id INT, title TEXT);
            CREATE TABLE authors (id REAL, name TEXT);
            INSERT INTO books VALUES (1,10,'A'),(2,10,'B'),(3,NULL,'C'),(4,99,'D');
            INSERT INTO authors VALUES (10.0,'X'),(10.00,'Y'),(NULL,'N');
            """);

        var result = (await db.ExecuteAsync("""
            SELECT B.title AS book, name FROM books AS b INNER JOIN authors a ON A.id = b.author_id
            ORDER BY b.id, a.name
            """))[0];
        Assert.Equal(new[] { "book", "name" }, result.Columns);
        Assert.Equal(new[] { "AX", "AY", "BX", "BY" }, result.Rows.Select(r => r[0].Display + r[1].Display));

        var page = (await db.ExecuteAsync("""
            SELECT b.title FROM books b JOIN authors a ON b.author_id = a.id
            WHERE a.name = 'Y' ORDER BY b.id DESC LIMIT 1 OFFSET 1
            """))[0];
        Assert.Equal("A", Assert.Single(Assert.Single(page.Rows)).Display);
        var count = (await db.ExecuteAsync("SELECT COUNT(*) FROM books JOIN authors ON books.author_id=authors.id"))[0];
        Assert.Equal("4", Assert.Single(Assert.Single(count.Rows)).Display);
        var noMatches = (await db.ExecuteAsync("SELECT COUNT(*) FROM books b JOIN authors a ON b.id=a.id"))[0];
        Assert.Equal("0", Assert.Single(Assert.Single(noMatches.Rows)).Display);
    }

    [Fact]
    public async Task Self_join_distinguishes_quoted_dots_and_expands_stars_in_source_order()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE [tree.nodes] (id INT, [parent.id] INT, label TEXT);
            INSERT INTO [tree.nodes] VALUES (1,NULL,'root'),(2,1,'child');
            """);
        const string from = " FROM [tree.nodes] AS [a.b] JOIN [tree.nodes] AS a ON [a.b].[parent.id] = a.id";
        var child = (await db.ExecuteAsync("SELECT [a.b].*, a.label AS parent" + from))[0];
        Assert.Equal(new[] { "id", "parent.id", "label", "parent" }, child.Columns);
        Assert.Equal(new[] { "2", "1", "child", "root" }, Assert.Single(child.Rows).Select(x => x.Display));
        var both = (await db.ExecuteAsync("SELECT *" + from))[0];
        Assert.Equal(new[] { "2", "1", "child", "1", "NULL", "root" }, Assert.Single(both.Rows).Select(x => x.Display));
        var ordinary = (await db.ExecuteAsync("SELECT [parent.id] FROM [tree.nodes] WHERE label='child'"))[0];
        Assert.Equal("1", Assert.Single(Assert.Single(ordinary.Rows)).Display);
    }

    [Theory]
    [InlineData("SELECT id FROM books b JOIN authors a ON b.id=a.id")]
    [InlineData("SELECT b.id FROM books b JOIN authors a ON b.id=a.id WHERE id=1")]
    [InlineData("SELECT b.id FROM books b JOIN authors a ON b.id=a.id ORDER BY id")]
    [InlineData("SELECT missing.id FROM books b JOIN authors a ON b.id=a.id")]
    [InlineData("SELECT b.id FROM books b JOIN authors a ON b.id=a.missing")]
    [InlineData("SELECT * FROM books AS dup JOIN authors AS DUP ON dup.id=dup.id")]
    public async Task Binding_rejects_ambiguous_or_invalid_references_even_for_empty_sources(string query)
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE books(id INT); CREATE TABLE authors(id INT)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(query));
    }

    [Fact]
    public async Task Join_validates_key_types_before_evaluating_rows()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE a(k TEXT); CREATE TABLE b(k INT)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM a JOIN b ON a.k=b.k"));
    }

    [Fact]
    public async Task Join_pair_limit_includes_boundary_and_precedes_result_pagination()
    {
        var db = Database();
        var values = string.Join(",", Enumerable.Repeat("(1)", 64));
        await db.ExecuteAsync($"CREATE TABLE a(k INT); CREATE TABLE b(k INT); INSERT INTO a VALUES {values}; INSERT INTO b VALUES {values}");
        const string from = " FROM a JOIN b ON a.k=b.k";
        var boundary = (await db.ExecuteAsync("SELECT COUNT(*)" + from))[0];
        Assert.Equal("4096", Assert.Single(Assert.Single(boundary.Rows)).Display);
        await db.ExecuteAsync("INSERT INTO b VALUES (1)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT a.k" + from + " LIMIT 1"));
    }

    [Fact]
    public async Task Joined_view_exposes_only_its_projected_columns_and_filtered_rows()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE books(id INT, shelf TEXT, visible BOOLEAN);
            INSERT INTO books VALUES (1,'a',TRUE),(2,'a',FALSE);
            CREATE VIEW public_books AS SELECT id AS book, shelf FROM books WHERE visible=TRUE;
            CREATE TABLE shelves(shelf TEXT, owner TEXT);
            INSERT INTO shelves VALUES ('a','private');
            """);
        foreach (var from in new[] {
                     "public_books p JOIN shelves s ON p.shelf=s.shelf",
                     "shelves s JOIN public_books p ON p.shelf=s.shelf" })
        {
            var result = (await db.ExecuteAsync("SELECT p.book FROM " + from))[0];
            Assert.Equal("1", Assert.Single(Assert.Single(result.Rows)).Display);
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT p.visible FROM " + from));
        }
        await Assert.ThrowsAsync<SqlException>(() => db.QueryPublishedViewAsync(
            "SELECT s.owner FROM public_books p JOIN shelves s ON p.shelf=s.shelf", ["public_books"]));
    }
}
