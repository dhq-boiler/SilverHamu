using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class TableEditHistoryTests
{
    private static async Task<(MemorySqlStore Store, SqlDatabase Db, TableEditSession Edit)> Open()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t (id INT PRIMARY KEY, title TEXT, note TEXT); INSERT INTO t VALUES (1,'one',NULL),(2,'two','');");
        var table = Assert.Single(await db.GetTablesAsync());
        var selected = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id;"));
        return (store, db, new(table, selected, "SELECT * FROM t ORDER BY id;", 1));
    }

    private static string Change(TableEditSession edit, int row, int column, string value, bool isNull = false)
    {
        var statement = edit.Stage(row, column, edit.ParseValue(column, value, isNull));
        edit.Synchronize(edit.Script + (edit.Script.Length == 0 ? "" : "\n") + statement);
        Assert.Null(edit.PreviewError);
        return statement;
    }

    [Fact]
    public async Task Undo_and_redo_restore_typed_values_sql_and_pending_cells_without_database_writes()
    {
        var (_, db, edit) = await Open();
        Assert.False(edit.CanUndo); Assert.False(edit.CanRedo);
        var sql = Change(edit, 0, 2, "");
        Assert.True(edit.IsModified(0, 2)); Assert.Equal(1, edit.ModifiedCellCount);
        Assert.Equal("", edit.Result.Rows[0][2].Text);
        Assert.Equal("", edit.Undo());
        Assert.True(edit.Result.Rows[0][2].IsNull); Assert.Equal(0, edit.ModifiedCellCount);
        Assert.False(edit.CanUndo); Assert.True(edit.CanRedo);
        Assert.Equal(sql, edit.Redo());
        Assert.Equal("", edit.Result.Rows[0][2].Text); Assert.True(edit.IsModified(0, 2));
        Assert.True(Assert.Single(await db.ExecuteAsync("SELECT note FROM t WHERE id=1")).Rows.Single()[0].IsNull);
    }

    [Theory]
    [InlineData(0)]
    [InlineData(1)]
    [InlineData(2)]
    public async Task Removing_a_generated_update_reverts_only_that_change_and_rebases_surviving_predicates(int removed)
    {
        var (store, db, edit) = await Open();
        string[] statements = [Change(edit, 0, 0, "11"), Change(edit, 0, 1, "changed"), Change(edit, 0, 2, "note")];
        edit.Synchronize(string.Join("\n", statements.Where((_, index) => index != removed)));
        Assert.Null(edit.PreviewError); Assert.Equal(2, edit.PendingCount); Assert.Equal(2, edit.ModifiedCellCount);
        Assert.Equal(removed == 0 ? "1" : "11", edit.Result.Rows[0][0].Text);
        Assert.Equal(removed == 1 ? "one" : "changed", edit.Result.Rows[0][1].Text);
        Assert.Equal(removed == 2, edit.Result.Rows[0][2].IsNull);
        Assert.False(edit.IsModified(0, removed));
        Assert.True(await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        var actual = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id;"));
        Assert.Contains(actual.Rows, row => row.SequenceEqual(edit.Result.Rows[0]));
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task Removing_one_of_two_edits_of_the_same_cell_preserves_the_surviving_value(bool removeFirst)
    {
        var (store, db, edit) = await Open();
        string first = Change(edit, 0, 1, "first"), second = Change(edit, 0, 1, "second");
        edit.Synchronize(removeFirst ? second : first);
        Assert.Null(edit.PreviewError); Assert.Equal(removeFirst ? "second" : "first", edit.Result.Rows[0][1].Text);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal(edit.Result.Rows[0][1], Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0]);
    }

    [Fact]
    public async Task Undoing_sql_deletion_restores_both_cells_and_redo_removes_the_change_again()
    {
        var (_, _, edit) = await Open();
        string first = Change(edit, 0, 1, "first"), second = Change(edit, 0, 2, "NULL");
        string complete = edit.Script;
        string reduced = edit.Synchronize(second);
        Assert.Equal("one", edit.Result.Rows[0][1].Text); Assert.Equal("NULL", edit.Result.Rows[0][2].Text);
        Assert.Equal(complete, edit.Undo()); Assert.Equal("first", edit.Result.Rows[0][1].Text);
        Assert.Equal(reduced, edit.Redo()); Assert.Equal("one", edit.Result.Rows[0][1].Text);
        Assert.DoesNotContain(first, reduced);
    }

    [Fact]
    public async Task All_deleted_or_commented_sql_restores_the_original_null_empty_and_text_values()
    {
        var (_, _, edit) = await Open();
        Change(edit, 0, 2, "NULL"); Change(edit, 1, 2, "", isNull: true);
        edit.Synchronize("-- no updates\n/* cleared */ ;");
        Assert.Null(edit.PreviewError); Assert.Equal(0, edit.ModifiedCellCount); Assert.Equal(0, edit.PendingCount);
        Assert.True(edit.Result.Rows[0][2].IsNull); Assert.Equal("", edit.Result.Rows[1][2].Text);
        edit.Undo(); Assert.Equal(2, edit.ModifiedCellCount);
        edit.Synchronize(""); Assert.Null(edit.PreviewError); Assert.Equal(0, edit.ModifiedCellCount);
    }

    [Fact]
    public async Task Comments_formatting_case_and_semicolons_inside_values_do_not_break_generated_operation_identity()
    {
        var (store, db, edit) = await Open();
        Change(edit, 0, 1, "first; -- 'quote'\nnext");
        string later = Change(edit, 0, 2, "second; /* value */");
        edit.Synchronize("/* retained */\n" + later.Replace("UPDATE ", "update /* format */ ").Replace(" SET ", "\nset\n"));
        Assert.Null(edit.PreviewError); Assert.Equal("one", edit.Result.Rows[0][1].Text);
        Assert.Equal("second; /* value */", edit.Result.Rows[0][2].Text);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal(edit.Result.Rows[0][2], Assert.Single(await db.ExecuteAsync("SELECT note FROM t WHERE id=1")).Rows.Single()[0]);
    }

    [Fact]
    public async Task Manually_changed_update_text_replays_assignments_using_the_original_row_values()
    {
        var (store, db, edit) = await Open();
        Change(edit, 0, 1, "staged");
        edit.Synchronize("UPDATE t SET title='manual', note=title WHERE id=1 RETURNING *;");
        Assert.Null(edit.PreviewError); Assert.Equal("manual", edit.Result.Rows[0][1].Text); Assert.Equal("one", edit.Result.Rows[0][2].Text);
        Assert.Equal(2, edit.ModifiedCellCount);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal(edit.Result.Rows[0], Assert.Single(await db.ExecuteAsync("SELECT * FROM t WHERE id=1")).Rows.Single());
    }

    [Theory]
    [InlineData("UPDATE t SET title=")]
    [InlineData("DELETE FROM t;")]
    [InlineData("UPDATE t SET title='all';")]
    [InlineData("UPDATE other SET title='no' WHERE id=1;")]
    [InlineData("UPDATE t SET title='none' WHERE id=999 RETURNING *;")]
    [InlineData("UPDATE t SET id=NULL WHERE id=1 RETURNING *;")]
    [InlineData("UPDATE t SET title='multiple' WHERE id>0 RETURNING *;")]
    public async Task Invalid_or_unpreviewable_sql_retains_the_last_valid_preview_and_can_be_undone(string invalid)
    {
        var (_, _, edit) = await Open();
        Change(edit, 0, 1, "valid"); var valid = edit.Script;
        edit.Synchronize(invalid);
        Assert.NotNull(edit.PreviewError); Assert.Equal("valid", edit.Result.Rows[0][1].Text); Assert.Equal(invalid, edit.Script);
        Assert.Equal(valid, edit.Undo()); Assert.Null(edit.PreviewError);
        Assert.Equal(invalid, edit.Redo()); Assert.NotNull(edit.PreviewError);
    }

    [Fact]
    public async Task A_new_edit_after_undo_discards_redo_and_histories_do_not_leak_between_sessions()
    {
        var (_, _, edit) = await Open(); var (_, _, other) = await Open();
        Change(edit, 0, 1, "first"); Change(edit, 1, 1, "second");
        edit.Undo(); Assert.True(edit.CanRedo);
        Change(edit, 0, 2, "branch");
        Assert.False(edit.CanRedo); Assert.Equal("two", edit.Result.Rows[1][1].Text);
        Assert.False(other.CanUndo); Assert.Equal(0, other.ModifiedCellCount);
    }

    [Fact]
    public async Task Returning_a_cell_to_its_loaded_value_clears_its_highlight_even_with_updates_still_queued()
    {
        var (_, _, edit) = await Open();
        Change(edit, 0, 1, "changed"); Change(edit, 0, 1, "one");
        Assert.False(edit.IsModified(0, 1)); Assert.Equal(0, edit.ModifiedCellCount); Assert.Equal(2, edit.PendingCount);
        edit.Undo(); Assert.True(edit.IsModified(0, 1));
    }

    [Fact]
    public async Task A_failed_batch_preserves_history_and_highlights_while_a_success_clears_them()
    {
        var (store, db, edit) = await Open();
        Change(edit, 0, 1, "staged");
        await db.ExecuteAsync("UPDATE t SET note='external' WHERE id=1;");
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.True(edit.CanUndo); Assert.True(edit.IsModified(0, 1));
        edit.Undo(); Assert.Equal(0, edit.ModifiedCellCount);
        edit.Redo(); Assert.True(edit.IsModified(0, 1));
        await db.ExecuteAsync("UPDATE t SET note=NULL WHERE id=1;");
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        edit.MarkApplied(); Assert.False(edit.CanUndo); Assert.False(edit.CanRedo); Assert.Equal(0, edit.ModifiedCellCount);
        Assert.Equal("staged", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0].Text);
        Assert.Throws<InvalidOperationException>(() => edit.Synchronize(""));
    }

    [Fact]
    public async Task Native_textbox_line_ending_normalization_does_not_add_history_or_destroy_redo()
    {
        var (_, _, edit) = await Open();
        Change(edit, 0, 1, "first"); Change(edit, 0, 2, "second");
        edit.Synchronize(edit.Script.ReplaceLineEndings("\r"));
        edit.Undo();
        Assert.Equal("first", edit.Result.Rows[0][1].Text); Assert.True(edit.Result.Rows[0][2].IsNull);
        Assert.True(edit.CanRedo);
        edit.Synchronize(edit.Script.ReplaceLineEndings("\r\n"));
        Assert.True(edit.CanRedo);
        edit.Redo(); Assert.Equal("second", edit.Result.Rows[0][2].Text);
    }

    [Fact]
    public void Statement_ranges_and_keys_preserve_values_and_ignore_only_sql_formatting()
    {
        const string script = "-- leading\n UPDATE t SET title='x;y' WHERE id=1; /* between */ UPDATE t SET note=U&'a\\000Ab' WHERE id=2;";
        var parts = SqlParser.ParseWithSource(script);
        Assert.Equal(2, parts.Count);
        Assert.Equal("UPDATE t SET title='x;y' WHERE id=1", script.Substring(parts[0].Start, parts[0].Length));
        Assert.Equal(parts[0].Key, Assert.Single(SqlParser.ParseWithSource("update t set title = 'x;y' where id = 1")).Key);
        Assert.NotEqual(parts[0].Key, Assert.Single(SqlParser.ParseWithSource("UPDATE t SET title='X;Y' WHERE id=1")).Key);
        Assert.Empty(SqlParser.ParseWithSource("-- empty\n"));
        Assert.Throws<SqlException>(() => SqlParser.Parse("-- empty\n"));
    }
}
