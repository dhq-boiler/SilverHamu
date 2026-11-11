using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class TableEditSessionTests
{
    private static async Task<(MemorySqlStore Store, SqlDatabase Db, TableEditSession Session)> Setup(string definition = "id INT PRIMARY KEY, title TEXT, note TEXT", string values = "(1,'one','A'),(2,'two',NULL)")
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync($"CREATE TABLE t ({definition}); INSERT INTO t VALUES {values};");
        var table = Assert.Single(await db.GetTablesAsync());
        const string select = "SELECT * FROM t ORDER BY id;";
        return (store, db, new(table, Assert.Single(await db.ExecuteAsync(select)), select, 7));
    }

    [Fact]
    public async Task Staging_changes_the_preview_and_adds_one_update_without_writing_the_database()
    {
        var (_, db, session) = await Setup();
        var sql = session.Stage(0, 1, session.ParseValue(1, "O'Brien; -- 日本語\nsecond\t\\end", false));
        Assert.IsType<UpdateRows>(Assert.Single(SqlParser.Parse(sql)));
        Assert.DoesNotContain('\n', sql); Assert.DoesNotContain('\r', sql);
        Assert.Contains("U&'", sql); Assert.Contains("RETURNING *", sql);
        Assert.Equal(1, session.PendingCount);
        Assert.Equal("one", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0].Text);
        Assert.Equal("O'Brien; -- 日本語\nsecond\t\\end", session.Result.Rows[0][1].Text);
    }

    [Fact]
    public async Task Multiple_cells_and_primary_key_changes_chain_the_current_row_values_and_commit_together()
    {
        var (store, db, session) = await Setup();
        string sql = session.Stage(0, 0, session.ParseValue(0, "11", false)) + "\n" +
            session.Stage(0, 1, session.ParseValue(1, "changed", false)) + "\n" + session.Stage(1, 2, session.ParseValue(2, "", false));
        Assert.Equal(3, session.PendingCount);
        Assert.True(await TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.False(store.InTransaction);
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal("", rows[0][2].Text); Assert.False(rows[0][2].IsNull);
        Assert.Equal("11", rows[1][0].Text); Assert.Equal("changed", rows[1][1].Text);
    }

    [Fact]
    public async Task Newlines_in_original_predicates_and_new_values_survive_a_single_line_update()
    {
        var (store, db, session) = await Setup(values: "(1,'line1\nline2','A'),(2,'two',NULL)");
        var sql = session.Stage(0, 1, session.ParseValue(1, "new\r\nvalue\t'quoted'", false));
        Assert.DoesNotContain('\n', sql); Assert.DoesNotContain('\r', sql);
        await TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("new\nvalue\t'quoted'", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task A_later_conflict_rolls_back_earlier_updates_and_preserves_the_external_change()
    {
        var (store, db, session) = await Setup();
        string sql = session.Stage(0, 1, session.ParseValue(1, "staged1", false)) + "\n" + session.Stage(1, 1, session.ParseValue(1, "staged2", false));
        await db.ExecuteAsync("UPDATE t SET note='external' WHERE id=2");
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction));
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal("one", rows[0][1].Text); Assert.Equal("two", rows[1][1].Text); Assert.Equal("external", rows[1][2].Text);
        Assert.Equal(2, session.PendingCount);
    }

    [Fact]
    public async Task Duplicate_rows_without_a_key_roll_back_the_whole_batch()
    {
        var (store, db, session) = await Setup("id INT, title TEXT, note TEXT", "(1,'one','A'),(2,'two',NULL),(2,'two',NULL)");
        string sql = session.Stage(0, 1, session.ParseValue(1, "first", false)) + "\n" + session.Stage(1, 1, session.ParseValue(1, "duplicate", false));
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction));
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal(new[] { "one", "two", "two" }, rows.Select(r => r[1].Text));
        Assert.False(store.InTransaction);
    }

    [Fact]
    public async Task A_unique_row_without_a_primary_key_can_be_edited_using_all_original_columns()
    {
        var (store, db, session) = await Setup("id INT, title TEXT, note TEXT");
        string sql = session.Stage(1, 1, session.ParseValue(1, "NULL", false));
        Assert.Contains("note IS NULL", sql);
        await TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("NULL", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=2")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task A_constraint_error_in_a_later_update_rolls_back_the_earlier_edit()
    {
        var (store, db, session) = await Setup("id INT PRIMARY KEY, title TEXT UNIQUE, note TEXT");
        string sql = session.Stage(0, 2, session.ParseValue(2, "changed", false)) + "\n" + session.Stage(1, 1, session.ParseValue(1, "one", false));
        await Assert.ThrowsAsync<SqlException>(() => TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.Equal("A", Assert.Single(await db.ExecuteAsync("SELECT note FROM t WHERE id=1")).Rows.Single()[0].Text);
        Assert.False(store.InTransaction);
    }

    [Fact]
    public async Task An_existing_transaction_keeps_control_of_commit_after_a_successful_batch()
    {
        var (store, db, session) = await Setup();
        await db.ExecuteAsync("BEGIN; INSERT INTO t VALUES (3,'prior','P');");
        var sql = session.Stage(0, 1, session.ParseValue(1, "edited", false));
        Assert.False(await TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.True(store.InTransaction);
        await db.ExecuteAsync("ROLLBACK;");
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal(2, rows.Count); Assert.Equal("one", rows[0][1].Text);
    }

    [Fact]
    public async Task Failed_batch_in_an_existing_transaction_preserves_work_before_the_savepoint()
    {
        var (store, db, session) = await Setup();
        await db.ExecuteAsync("BEGIN; INSERT INTO t VALUES (3,'prior','P'); UPDATE t SET note='external' WHERE id=2;");
        var sql = session.Stage(0, 1, session.ParseValue(1, "edited", false)) + "\n" + session.Stage(1, 1, session.ParseValue(1, "conflict", false));
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.True(store.InTransaction);
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal(3, rows.Count); Assert.Equal("one", rows[0][1].Text); Assert.Equal("prior", rows[2][1].Text);
        await db.ExecuteAsync("ROLLBACK;");
    }

    [Theory]
    [InlineData("")]
    [InlineData("-- no updates")]
    [InlineData("SELECT * FROM t;")]
    [InlineData("DELETE FROM t;")]
    [InlineData("UPDATE other SET title='x' WHERE id=1 RETURNING *;")]
    [InlineData("UPDATE t SET title='x' RETURNING *;")]
    [InlineData("UPDATE t SET title='x' WHERE id=1 RETURNING *; COMMIT;")]
    public async Task Non_dml_other_targets_and_unbounded_changes_are_rejected_before_any_request(string sql)
    {
        int calls = 0;
        var error = await Record.ExceptionAsync(() => TableEditBatch.ExecuteAsync(sql, "t", _ => { calls++; return Task.FromResult<IReadOnlyList<SqlResult>>([]); }, () => false));
        Assert.True(error is InvalidOperationException or SqlException);
        Assert.Equal(0, calls);
    }

    [Fact]
    public async Task Removing_returning_cannot_commit_an_unchecked_update()
    {
        var (store, db, _) = await Setup();
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync("UPDATE t SET title='changed' WHERE id=1;", "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.Equal("one", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Unchanged_values_generate_no_update_and_applied_sessions_require_reload()
    {
        var (_, _, session) = await Setup();
        Assert.Equal("", session.Stage(0, 1, session.ParseValue(1, "one", false)));
        Assert.Equal(0, session.PendingCount);
        session.Stage(0, 1, session.ParseValue(1, "new", false));
        session.MarkApplied(); Assert.Equal(0, session.PendingCount);
        Assert.Throws<InvalidOperationException>(() => session.Stage(0, 1, SqlValue.String("again")));
    }

    [Theory]
    [InlineData("INTEGER", "9223372036854775807", "NUMBER", "9223372036854775807")]
    [InlineData("INTEGER", "-9223372036854775808", "NUMBER", "-9223372036854775808")]
    [InlineData("DECIMAL(5,2)", "12.50", "NUMBER", "12.50")]
    [InlineData("BOOLEAN", "false", "BOOLEAN", "false")]
    [InlineData("BLOB", "0x00fF", "BLOB", "00FF")]
    [InlineData("TEXT", "", "TEXT", "")]
    [InlineData("TEXT", "NULL", "TEXT", "NULL")]
    [InlineData("DATE", "2026-11-12", "DATE", "2026-11-12")]
    [InlineData("UUID", "de305d54-75b4-431b-adb2-eb6b9e546014", "UUID", "de305d54-75b4-431b-adb2-eb6b9e546014")]
    public void Values_keep_their_column_type_and_exact_representation(string type, string input, string kind, string text)
    {
        var table = new SqlTable("typed", [new("value", type, false, false, false, null, null)]);
        var session = new TableEditSession(table, new(["value"], [[SqlValue.Null]], ""), "SELECT * FROM typed;", 1);
        var value = session.ParseValue(0, input, false);
        Assert.Equal(kind, value.Kind); Assert.Equal(text, value.Text);
        Assert.IsType<UpdateRows>(Assert.Single(SqlParser.Parse(session.Stage(0, 0, value))));
    }

    [Theory]
    [InlineData("INTEGER", "1.5")]
    [InlineData("INTEGER", "9223372036854775808")]
    [InlineData("DECIMAL(5,2)", "1.234")]
    [InlineData("REAL", "NaN")]
    [InlineData("BLOB", "0x123")]
    [InlineData("BOOLEAN", "maybe")]
    [InlineData("DATE", "2026-02-30")]
    public void Invalid_or_lossy_typed_values_are_not_staged(string type, string input)
    {
        var table = new SqlTable("typed", [new("value", type, false, false, false, null, null)]);
        var session = new TableEditSession(table, new(["value"], [[SqlValue.Null]], ""), "SELECT * FROM typed;", 1);
        Assert.ThrowsAny<Exception>(() => session.ParseValue(0, input, false));
        Assert.Equal(0, session.PendingCount);
    }

    [Fact]
    public async Task Nulls_are_explicit_and_cannot_be_assigned_to_required_columns()
    {
        var (_, _, session) = await Setup();
        Assert.True(session.ParseValue(1, "ignored", true).IsNull);
        Assert.Throws<ArgumentException>(() => session.ParseValue(0, "", true));
    }

    [Fact]
    public async Task Distinct_types_are_preserved_in_both_predicates_and_assignments()
    {
        var store = new MemorySqlStore(); var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TYPE Money AS DISTINCT DECIMAL(8,2) CHECK (VALUE >= 0); CREATE TABLE t(id INT PRIMARY KEY, price Money); INSERT INTO t VALUES (1,12.50);");
        var session = new TableEditSession(Assert.Single(await db.GetTablesAsync()), Assert.Single(await db.ExecuteAsync("SELECT * FROM t")), "SELECT * FROM t", 1);
        var sql = session.Stage(0, 1, session.ParseValue(1, "15.75", false));
        Assert.Contains("AS Money", sql);
        await TableEditBatch.ExecuteAsync(sql, "t", db.ExecuteAsync, () => store.InTransaction);
        var price = Assert.Single(await db.ExecuteAsync("SELECT price FROM t")).Rows.Single()[0];
        Assert.Equal("Money", price.TypeName); Assert.Equal("15.75", price.Text);
    }

    [Fact]
    public async Task Names_requiring_quotes_and_string_injection_are_treated_as_data()
    {
        var store = new MemorySqlStore(); var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE \"Order\" (\"Key Value\" INT PRIMARY KEY, \"Odd\"\"Name\" TEXT); INSERT INTO \"Order\" VALUES (1,'before');");
        var table = Assert.Single(await db.GetTablesAsync());
        var session = new TableEditSession(table, Assert.Single(await db.ExecuteAsync("SELECT * FROM \"Order\"")), "", 1);
        var value = "'; DELETE FROM \"Order\"; --";
        var sql = session.Stage(0, 1, session.ParseValue(1, value, false));
        await TableEditBatch.ExecuteAsync(sql, "Order", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal(value, Assert.Single(await db.ExecuteAsync("SELECT * FROM \"Order\"")).Rows.Single()[1].Text);
    }
}
