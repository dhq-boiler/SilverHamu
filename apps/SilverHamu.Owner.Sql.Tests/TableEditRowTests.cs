using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class TableEditRowTests
{
    private static async Task<(MemorySqlStore Store, SqlDatabase Db, TableEditSession Edit)> Open(string schema = "id INT PRIMARY KEY, title TEXT NOT NULL, note TEXT DEFAULT 'default'")
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync($"CREATE TABLE t ({schema}); INSERT INTO t VALUES (1,'one',NULL),(2,'two','');");
        var table = Assert.Single(await db.GetTablesAsync());
        return (store, db, new(table, Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id;")), "SELECT * FROM t ORDER BY id;", 1));
    }

    private static void Fill(TableEditSession edit, int row, int column, string value, bool isNull = false)
    {
        edit.ChangeInsertedCell(row, column, edit.ParseValue(column, value, isNull));
        Assert.Null(edit.PreviewError);
    }

    private static void Update(TableEditSession edit, int row, int column, string value)
    {
        string statement = edit.Stage(row, column, edit.ParseValue(column, value, false));
        edit.Synchronize(edit.Script + (edit.Script.Length > 0 ? "\n" : "") + statement);
        Assert.Null(edit.PreviewError);
    }

    [Fact]
    public async Task Added_rows_start_with_defaults_and_required_placeholders_without_database_writes()
    {
        var (_, db, edit) = await Open();
        edit.AddRow();
        Assert.Equal(3, edit.Result.Rows.Count); Assert.True(edit.IsAdded(2)); Assert.Equal("+1", edit.RowLabel(2));
        Assert.Equal(1, edit.IncompleteRowCount); Assert.Equal(1, edit.AddedRowCount);
        Assert.Equal("default", edit.Result.Rows[2][2].Text);
        Assert.IsAssignableFrom<InsertStatement>(Assert.Single(SqlParser.Parse(edit.Script)));
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows.Count);
    }

    [Fact]
    public async Task Filling_an_added_row_rewrites_one_insert_and_commits_typed_null_empty_and_quoted_values()
    {
        var (store, db, edit) = await Open();
        edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "O'Brien; -- 日本語\nline"); Fill(edit, 2, 2, "");
        Assert.Equal(0, edit.IncompleteRowCount); Assert.Equal(1, edit.PendingCount);
        Assert.IsAssignableFrom<InsertStatement>(Assert.Single(SqlParser.Parse(edit.Script)));
        Assert.DoesNotContain('\n', edit.Script);
        Assert.True(await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        var added = Assert.Single(await db.ExecuteAsync("SELECT * FROM t WHERE id=3")).Rows.Single();
        Assert.Equal("O'Brien; -- 日本語\nline", added[1].Text); Assert.Equal("", added[2].Text); Assert.False(added[2].IsNull);
        edit.MarkApplied(); Assert.Equal(0, edit.AddedRowCount); Assert.False(edit.IsAdded(2)); Assert.False(edit.CanUndo);
    }

    [Fact]
    public async Task Insert_cell_edits_and_row_creation_are_separate_undo_steps()
    {
        var (_, _, edit) = await Open();
        edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "three");
        edit.Undo(); Assert.True(edit.Result.Rows[2][1].IsNull); Assert.Equal("3", edit.Result.Rows[2][0].Text); Assert.Equal(1, edit.IncompleteRowCount);
        edit.Undo(); Assert.True(edit.Result.Rows[2][0].IsNull);
        edit.Undo(); Assert.Equal(2, edit.Result.Rows.Count); Assert.Equal(0, edit.AddedRowCount);
        edit.Redo(); edit.Redo(); edit.Redo(); Assert.Equal("three", edit.Result.Rows[2][1].Text);
    }

    [Theory]
    [InlineData(2)]
    [InlineData(3)]
    public async Task Removing_one_of_two_identical_draft_rows_keeps_the_other_row_editable(int removed)
    {
        var (_, _, edit) = await Open();
        edit.AddRow(); edit.AddRow(); Assert.Equal(2, edit.AddedRowCount);
        edit.DeleteRows([removed]);
        Assert.Null(edit.PreviewError); Assert.Equal(3, edit.Result.Rows.Count); Assert.True(edit.IsAdded(2));
        Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "survivor");
        Assert.Equal(1, edit.PendingCount); Assert.Equal("survivor", edit.Result.Rows[2][1].Text);
        edit.Undo(); edit.Undo(); edit.Undo(); Assert.Equal(4, edit.Result.Rows.Count); Assert.Equal(2, edit.AddedRowCount);
    }

    [Fact]
    public async Task Deleting_an_unapplied_insert_cancels_it_and_undo_restores_the_complete_row()
    {
        var (_, db, edit) = await Open();
        edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "three");
        edit.DeleteRows([2]);
        Assert.Equal("", edit.Script);
        Assert.Equal(0, edit.PendingCount); Assert.Equal(0, edit.AddedRowCount); Assert.Equal(2, edit.Result.Rows.Count);
        Assert.DoesNotContain("DELETE", edit.Script);
        edit.Undo(); Assert.Equal("three", edit.Result.Rows[2][1].Text);
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows.Count);
    }

    [Fact]
    public async Task Deleting_existing_rows_keeps_tombstones_and_one_delete_per_row_until_execution()
    {
        var (store, db, edit) = await Open();
        edit.DeleteRows([0, 1, 0]);
        Assert.Equal(2, edit.DeletedRowCount); Assert.Equal(2, edit.PendingCount); Assert.Equal(2, edit.Result.Rows.Count);
        Assert.All(SqlParser.Parse(edit.Script), statement => Assert.IsType<DeleteRows>(statement));
        Assert.Equal("−1", edit.RowLabel(0)); Assert.True(edit.IsDeleted(0)); Assert.True(edit.IsDeleted(1));
        Assert.Throws<InvalidOperationException>(() => edit.Stage(0, 1, SqlValue.String("forbidden")));
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows.Count);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Empty(Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows);
        edit.MarkApplied(); Assert.Empty(edit.Result.Rows); Assert.Equal(0, edit.DeletedRowCount);
    }

    [Fact]
    public async Task Removing_delete_sql_or_undoing_it_restores_the_row_and_its_prior_cell_edits()
    {
        var (_, _, edit) = await Open();
        Update(edit, 0, 1, "changed"); string update = edit.Script;
        edit.DeleteRows([0]); Assert.True(edit.IsDeleted(0)); Assert.False(edit.IsModified(0, 1));
        edit.Synchronize(update); Assert.False(edit.IsDeleted(0)); Assert.True(edit.IsModified(0, 1)); Assert.Equal("changed", edit.Result.Rows[0][1].Text);
        edit.Undo(); Assert.True(edit.IsDeleted(0)); edit.Undo(); Assert.False(edit.IsDeleted(0));
    }

    [Fact]
    public async Task Removing_an_earlier_update_rebases_a_surviving_delete_to_the_loaded_values()
    {
        var (store, db, edit) = await Open();
        Update(edit, 0, 0, "11"); edit.DeleteRows([0]);
        var part = SqlParser.ParseWithSource(edit.Script).Last();
        edit.Synchronize(edit.Script.Substring(part.Start, part.Length) + ";");
        Assert.Null(edit.PreviewError); Assert.True(edit.IsDeleted(0)); Assert.Contains("id = 1 AND", edit.Script);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("2", Assert.Single(await db.ExecuteAsync("SELECT id FROM t")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Mixed_insert_update_and_delete_apply_atomically_and_preserve_preview_identity()
    {
        var (store, db, edit) = await Open();
        edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "three");
        Update(edit, 0, 1, "changed"); edit.DeleteRows([1]);
        Assert.Equal(3, edit.PendingCount); Assert.Equal(1, edit.AddedRowCount); Assert.Equal(1, edit.DeletedRowCount);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal(2, rows.Count); Assert.Equal("changed", rows[0][1].Text); Assert.Equal("three", rows[1][1].Text);
        edit.MarkApplied(); Assert.Equal(2, edit.Result.Rows.Count); Assert.False(edit.CanRedo); Assert.Equal(0, edit.ModifiedCellCount);
    }

    [Fact]
    public async Task A_conflicting_delete_rolls_back_prior_insert_and_update_and_retains_all_pending_operations()
    {
        var (store, db, edit) = await Open();
        edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "three"); Update(edit, 0, 1, "changed"); edit.DeleteRows([1]);
        await db.ExecuteAsync("UPDATE t SET title='external' WHERE id=2");
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal(2, rows.Count); Assert.Equal("one", rows[0][1].Text); Assert.Equal("external", rows[1][1].Text);
        Assert.True(edit.IsDeleted(1)); Assert.True(edit.IsAdded(2)); Assert.Equal(3, edit.PendingCount); Assert.True(edit.CanUndo);
    }

    [Fact]
    public async Task A_duplicate_insert_rolls_back_a_prior_delete_in_the_same_batch()
    {
        var (store, db, edit) = await Open();
        edit.DeleteRows([1]); edit.AddRow(); Fill(edit, 2, 0, "1"); Fill(edit, 2, 1, "duplicate");
        await Assert.ThrowsAsync<SqlException>(() => TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows.Count); Assert.False(store.InTransaction);
    }

    [Fact]
    public async Task Mixed_changes_use_a_savepoint_and_leave_the_outer_transaction_open()
    {
        var (store, db, edit) = await Open();
        await db.ExecuteAsync("BEGIN;");
        edit.DeleteRows([0]); edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "three");
        Assert.False(await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.True(store.InTransaction); await db.ExecuteAsync("ROLLBACK;");
        Assert.Equal("one", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0].Text);
        Assert.Empty(Assert.Single(await db.ExecuteAsync("SELECT * FROM t WHERE id=3")).Rows);
    }

    [Fact]
    public async Task Handwritten_insert_and_delete_join_the_same_preview_and_can_be_edited_in_the_grid()
    {
        var (store, db, edit) = await Open();
        edit.Synchronize("INSERT INTO t (id,title) VALUES (3,'manual') RETURNING *; DELETE FROM t WHERE id=1 RETURNING *;");
        Assert.Null(edit.PreviewError); Assert.Equal(1, edit.AddedRowCount); Assert.True(edit.IsDeleted(0)); Assert.Equal("default", edit.Result.Rows[2][2].Text);
        Fill(edit, 2, 1, "grid");
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("grid", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=3")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Deleting_insert_sql_restores_the_original_row_count_and_undo_restores_the_insert()
    {
        var (_, _, edit) = await Open();
        edit.AddRow(); Fill(edit, 2, 0, "3"); Fill(edit, 2, 1, "three");
        edit.Synchronize(""); Assert.Equal(2, edit.Result.Rows.Count); Assert.Equal(0, edit.AddedRowCount);
        edit.Undo(); Assert.Equal(3, edit.Result.Rows.Count); Assert.Equal("three", edit.Result.Rows[2][1].Text);
    }

    [Fact]
    public async Task Cancelling_a_draft_does_not_shift_other_insert_identities_or_existing_update_targets()
    {
        var (store, db, edit) = await Open();
        edit.AddRow(); edit.AddRow(); Fill(edit, 3, 0, "4"); Fill(edit, 3, 1, "four");
        edit.DeleteRows([2]); Update(edit, 1, 1, "changed-two"); Fill(edit, 2, 2, "survives");
        Assert.Equal("+2", edit.RowLabel(2));
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("changed-two", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=2")).Rows.Single()[0].Text);
        Assert.Equal("survives", Assert.Single(await db.ExecuteAsync("SELECT note FROM t WHERE id=4")).Rows.Single()[0].Text);
    }

    [Theory]
    [InlineData("DELETE FROM t RETURNING *;")]
    [InlineData("DELETE FROM other WHERE id=1 RETURNING *;")]
    [InlineData("INSERT INTO other VALUES (3,'x',NULL) RETURNING *;")]
    [InlineData("INSERT INTO t VALUES (3,'x',NULL),(4,'y',NULL) RETURNING *;")]
    [InlineData("INSERT INTO t SELECT * FROM t;")]
    [InlineData("TRUNCATE TABLE t;")]
    public async Task Unsafe_or_unpreviewable_row_scripts_are_rejected_before_database_requests(string sql)
    {
        int requests = 0;
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(sql, "t", _ => { requests++; return Task.FromResult<IReadOnlyList<SqlResult>>([]); }, () => false));
        Assert.Equal(0, requests);
        var (_, _, edit) = await Open(); edit.Synchronize(sql); Assert.NotNull(edit.PreviewError);
    }

    [Fact]
    public async Task Draft_nulls_are_allowed_only_until_required_single_and_composite_keys_are_filled()
    {
        var (_, _, edit) = await Open("id INT, title TEXT, note TEXT, PRIMARY KEY(id,title)");
        edit.AddRow(); Assert.Equal(1, edit.IncompleteRowCount);
        Fill(edit, 2, 0, "3"); Assert.Equal(1, edit.IncompleteRowCount);
        Fill(edit, 2, 1, "three"); Assert.Equal(0, edit.IncompleteRowCount);
        Assert.Throws<ArgumentException>(() => edit.ParseValue(0, "", true));
    }

    [Fact]
    public async Task The_first_row_can_be_added_to_an_empty_result_and_committed()
    {
        var (store, db, existing) = await Open();
        await db.ExecuteAsync("DELETE FROM t;");
        var edit = new TableEditSession(existing.Table, Assert.Single(await db.ExecuteAsync("SELECT * FROM t;")), "SELECT * FROM t;", 1);
        edit.AddRow(); Fill(edit, 0, 0, "1"); Fill(edit, 0, 1, "first");
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("first", Assert.Single(await db.ExecuteAsync("SELECT title FROM t")).Rows.Single()[0].Text);
        edit.MarkApplied(); Assert.Single(edit.Result.Rows); Assert.Equal(0, edit.AddedRowCount);
    }

    [Fact]
    public async Task Deleting_an_indistinguishable_keyless_row_rolls_back_when_multiple_rows_match()
    {
        var (store, db, existing) = await Open("id INT, title TEXT, note TEXT");
        await db.ExecuteAsync("INSERT INTO t VALUES (1,'one',NULL);");
        var edit = new TableEditSession(existing.Table, Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id;")), "SELECT * FROM t;", 1);
        edit.DeleteRows([0]);
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.Equal(3, Assert.Single(await db.ExecuteAsync("SELECT * FROM t;")).Rows.Count);
        Assert.True(edit.IsDeleted(0)); Assert.True(edit.CanUndo);
    }

    [Fact]
    public async Task Removing_returning_from_delete_cannot_commit_an_unchecked_deletion()
    {
        var (store, db, _) = await Open();
        await Assert.ThrowsAsync<InvalidOperationException>(() => TableEditBatch.ExecuteAsync("DELETE FROM t WHERE id=1;", "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t;")).Rows.Count);
    }

    [Fact]
    public void Partial_cell_selections_map_to_distinct_selected_rows_for_deletion()
    {
        var selection = new ResultSelection(); selection.Reset(4, 3);
        selection.SelectCell(0, 1); selection.SelectCell(2, 2, extend: true); selection.SelectRow(3, toggle: true);
        Assert.Equal([0, 1, 2, 3], selection.SelectedRows());
    }
}
