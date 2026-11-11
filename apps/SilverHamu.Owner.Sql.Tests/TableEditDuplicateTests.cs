using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class TableEditDuplicateTests
{
    private static async Task<(MemorySqlStore Store, SqlDatabase Db, TableEditSession Edit)> Open()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t (id INT PRIMARY KEY, title TEXT NOT NULL, note TEXT, bytes BLOB, amount DECIMAL(10,2)); " +
            "INSERT INTO t VALUES (1,U&'O''Brien; -- 日本語\\000Aline',NULL,X'00FF',10.25),(2,'two','',X'',20.50);");
        return (store, db, new(Assert.Single(await db.GetTablesAsync()), Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")), "SELECT * FROM t ORDER BY id", 1));
    }

    [Fact]
    public async Task Duplicating_clears_the_primary_key_and_preserves_other_typed_values_without_writes()
    {
        var (_, db, edit) = await Open();
        var original = (SqlValue[])edit.Result.Rows[0].Clone();
        edit.DuplicateRows([0]);
        Assert.Null(edit.PreviewError); Assert.Equal(3, edit.Result.Rows.Count); Assert.True(edit.IsAdded(2));
        Assert.True(edit.Result.Rows[2][0].IsNull); Assert.Equal(original.Skip(1), edit.Result.Rows[2].Skip(1));
        Assert.Equal(original, edit.Result.Rows[0]); Assert.Equal(1, edit.IncompleteRowCount); Assert.Equal(1, edit.AddedRowCount); Assert.Equal(1, edit.PendingCount);
        Assert.IsAssignableFrom<InsertStatement>(Assert.Single(SqlParser.Parse(edit.Script)));
        Assert.DoesNotContain('\n', edit.Script);
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows.Count);
    }

    [Fact]
    public async Task Multiple_selected_rows_are_copied_once_in_table_order_as_one_undo_step()
    {
        var (_, _, edit) = await Open();
        edit.DuplicateRows([1, 0, 1]);
        Assert.Equal(4, edit.Result.Rows.Count); Assert.Equal(2, edit.AddedRowCount); Assert.Equal(2, edit.PendingCount);
        Assert.True(edit.Result.Rows[2][0].IsNull); Assert.True(edit.Result.Rows[3][0].IsNull);
        Assert.Equal(edit.Result.Rows[0][1], edit.Result.Rows[2][1]); Assert.Equal(edit.Result.Rows[1][1], edit.Result.Rows[3][1]);
        edit.Undo(); Assert.Equal(2, edit.Result.Rows.Count); Assert.Equal(0, edit.AddedRowCount);
        edit.Redo(); Assert.Equal(4, edit.Result.Rows.Count); Assert.Equal(2, edit.PendingCount);
    }

    [Fact]
    public async Task Pending_cell_changes_are_copied_instead_of_the_database_snapshot()
    {
        var (_, db, edit) = await Open();
        edit.Synchronize(edit.Stage(0, 1, edit.ParseValue(1, "pending", false)));
        edit.DuplicateRows([0]);
        Assert.Equal("pending", edit.Result.Rows[2][1].Text);
        Assert.NotEqual("pending", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=1")).Rows.Single()[0].Text);
        edit.Undo(); Assert.Equal("pending", edit.Result.Rows[0][1].Text); Assert.Equal(1, edit.PendingCount);
    }

    [Fact]
    public async Task Duplicate_keys_rollback_the_batch_and_the_new_key_can_be_changed_before_retrying()
    {
        var (store, db, edit) = await Open();
        edit.DuplicateRows([0]);
        edit.ChangeInsertedCell(2, 0, edit.ParseValue(0, "1", false));
        await Assert.ThrowsAsync<SqlException>(() => TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction));
        Assert.Equal(2, Assert.Single(await db.ExecuteAsync("SELECT * FROM t")).Rows.Count);
        Assert.True(edit.IsAdded(2)); Assert.True(edit.CanUndo);
        edit.ChangeInsertedCell(2, 0, edit.ParseValue(0, "3", false));
        Assert.Equal(1, edit.PendingCount);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        var rows = Assert.Single(await db.ExecuteAsync("SELECT * FROM t ORDER BY id")).Rows;
        Assert.Equal(3, rows.Count); Assert.Equal(rows[0].Skip(1), rows[2].Skip(1));
    }

    [Fact]
    public async Task Duplicating_an_added_row_creates_an_independent_editable_insert()
    {
        var (store, db, edit) = await Open();
        edit.DuplicateRows([1]); edit.ChangeInsertedCell(2, 0, edit.ParseValue(0, "3", false));
        edit.DuplicateRows([2]); edit.ChangeInsertedCell(3, 0, edit.ParseValue(0, "4", false));
        edit.ChangeInsertedCell(2, 1, edit.ParseValue(1, "source changed", false));
        Assert.Equal("two", edit.Result.Rows[3][1].Text); Assert.Equal("source changed", edit.Result.Rows[2][1].Text);
        Assert.Equal(2, edit.PendingCount);
        await TableEditBatch.ExecuteAsync(edit.Script, "t", db.ExecuteAsync, () => store.InTransaction);
        Assert.Equal("two", Assert.Single(await db.ExecuteAsync("SELECT title FROM t WHERE id=4")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Removing_a_duplicate_insert_or_deleting_its_pending_row_preserves_the_source()
    {
        var (_, _, edit) = await Open();
        edit.DuplicateRows([0]); edit.DuplicateRows([1]);
        var remaining = SqlParser.ParseWithSource(edit.Script).Last();
        edit.Synchronize(edit.Script.Substring(remaining.Start, remaining.Length) + ";");
        Assert.Equal(3, edit.Result.Rows.Count); Assert.True(edit.Result.Rows[2][0].IsNull); Assert.Equal("two", edit.Result.Rows[2][1].Text);
        edit.Undo(); Assert.Equal(4, edit.Result.Rows.Count);
        edit.DeleteRows([2, 3]); Assert.Equal(2, edit.Result.Rows.Count); Assert.Equal(0, edit.PendingCount);
        Assert.Equal("1", edit.Result.Rows[0][0].Text); Assert.Equal("2", edit.Result.Rows[1][0].Text);
        edit.Undo(); Assert.Equal(4, edit.Result.Rows.Count);
    }

    [Fact]
    public async Task Deleted_rows_are_excluded_and_an_empty_selection_adds_no_history()
    {
        var (_, _, edit) = await Open();
        Assert.Equal("", edit.DuplicateRows([])); Assert.False(edit.CanUndo);
        edit.DeleteRows([0]); edit.DuplicateRows([0, 1]);
        Assert.Equal(1, edit.AddedRowCount); Assert.True(edit.Result.Rows[2][0].IsNull); Assert.Equal("two", edit.Result.Rows[2][1].Text);
        edit.Undo(); Assert.Equal(2, edit.Result.Rows.Count); Assert.True(edit.IsDeleted(0));
    }

    [Fact]
    public async Task Composite_keys_and_unique_indexes_are_cleared_but_ordinary_index_values_are_preserved()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE keyed (a INT, b INT, code TEXT UNIQUE, u1 TEXT, u2 TEXT, conditional TEXT, plain TEXT, payload TEXT, PRIMARY KEY(a,b)); " +
            "CREATE UNIQUE INDEX pair_unique ON keyed(u1,u2); CREATE UNIQUE INDEX partial_unique ON keyed(conditional) WHERE a>0; CREATE INDEX ordinary ON keyed(plain); " +
            "INSERT INTO keyed VALUES (1,2,'code','left','right','conditional','indexed','keep');");
        var edit = new TableEditSession(Assert.Single(await db.GetTablesAsync()), Assert.Single(await db.ExecuteAsync("SELECT * FROM keyed")), "SELECT * FROM keyed", 1);
        edit.DuplicateRows([0]);
        Assert.Null(edit.PreviewError);
        Assert.All(edit.Result.Rows[1].Take(6), value => Assert.True(value.IsNull));
        Assert.Equal("indexed", edit.Result.Rows[1][6].Text); Assert.Equal("keep", edit.Result.Rows[1][7].Text);
        Assert.Equal("1", edit.Result.Rows[0][0].Text); Assert.Equal(1, edit.IncompleteRowCount);
        edit.Undo(); edit.Redo(); Assert.All(edit.Result.Rows[1].Take(6), value => Assert.True(value.IsNull));
    }

    [Fact]
    public async Task Required_unique_columns_need_input_while_nullable_unique_columns_may_remain_null()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE keyed (id INT PRIMARY KEY, code TEXT NOT NULL UNIQUE DEFAULT 'default', nullable_code TEXT UNIQUE, payload TEXT); " +
            "INSERT INTO keyed VALUES (1,'original','nullable','keep');");
        var edit = new TableEditSession(Assert.Single(await db.GetTablesAsync()), Assert.Single(await db.ExecuteAsync("SELECT * FROM keyed")), "SELECT * FROM keyed", 1);
        edit.DuplicateRows([0]);
        Assert.All(edit.Result.Rows[1].Take(3), value => Assert.True(value.IsNull));
        edit.ChangeInsertedCell(1, 0, edit.ParseValue(0, "2", false)); Assert.Equal(1, edit.IncompleteRowCount);
        edit.ChangeInsertedCell(1, 1, edit.ParseValue(1, "new", false)); Assert.Equal(0, edit.IncompleteRowCount);
        Assert.True(edit.Result.Rows[1][2].IsNull);
        await TableEditBatch.ExecuteAsync(edit.Script, "keyed", db.ExecuteAsync, () => store.InTransaction);
        var added = Assert.Single(await db.ExecuteAsync("SELECT * FROM keyed WHERE id=2")).Rows.Single();
        Assert.Equal("new", added[1].Text); Assert.True(added[2].IsNull); Assert.Equal("keep", added[3].Text);
    }

    [Fact]
    public async Task A_pending_primary_key_change_is_preserved_in_the_source_and_cleared_in_its_duplicate()
    {
        var (_, _, edit) = await Open();
        edit.Synchronize(edit.Stage(0, 0, edit.ParseValue(0, "11", false)));
        edit.DuplicateRows([0]);
        Assert.Equal("11", edit.Result.Rows[0][0].Text); Assert.True(edit.Result.Rows[2][0].IsNull);
        Assert.Equal(2, edit.PendingCount); Assert.Equal(1, edit.IncompleteRowCount);
        edit.Undo(); Assert.Equal("11", edit.Result.Rows[0][0].Text); Assert.Equal(2, edit.Result.Rows.Count);
    }

    [Fact]
    public async Task Duplicated_required_drafts_remain_incomplete_and_keep_separate_identities()
    {
        var (_, _, edit) = await Open();
        edit.AddRow(); edit.DuplicateRows([2]);
        Assert.Equal(2, edit.IncompleteRowCount);
        Assert.NotEqual(edit.RowLabel(2), edit.RowLabel(3));
        edit.ChangeInsertedCell(2, 0, edit.ParseValue(0, "3", false));
        edit.ChangeInsertedCell(2, 1, edit.ParseValue(1, "complete", false));
        Assert.Equal(1, edit.IncompleteRowCount); Assert.True(edit.Result.Rows[3][0].IsNull);
    }
}
