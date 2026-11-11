using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class SecondaryIndexTests
{
    [Fact]
    public async Task Reindex_repairs_missing_entries_for_each_index_on_a_table_without_a_primary_key()
    {
        var store = new MemorySqlStore();
        var entries = new List<string>();
        var prefix = SqlNames.Hex("_catdb_owner/sql/v1/index/");
        var db = new SqlDatabase(request =>
        {
            if (request["op"]!.ToString() == "put" && request["key"]!.ToString().StartsWith(prefix, StringComparison.Ordinal))
                entries.Add(request["key"]!.ToString());
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t(note TEXT, active BOOLEAN); INSERT INTO t VALUES ('猫',TRUE); CREATE INDEX first ON t(note); CREATE INDEX second ON t(active)");
        Assert.Equal(2, entries.Count);
        await db.ExecuteAsync("BEGIN");
        foreach (var key in entries)
            await store.RequestAsync(new() { ["op"] = "delete", ["key"] = key });
        await db.ExecuteAsync("COMMIT");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM t WHERE note='猫'"))[0].Rows);
        await db.ExecuteAsync("REINDEX t");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t WHERE note='猫'"))[0].Rows);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t WHERE active=TRUE"))[0].Rows);
    }

    [Fact]
    public async Task Equality_lookup_pages_duplicate_keys_and_fetches_only_matching_rows()
    {
        var store = new MemorySqlStore();
        var rowScans = 0;
        var rowPrefix = SqlNames.Hex("_catdb_owner/sql/v1/row/");
        var db = new SqlDatabase(request =>
        {
            if (request["op"]!.ToString() == "scan" && request["start"]!.ToString().StartsWith(rowPrefix, StringComparison.Ordinal))
                rowScans++;
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE loans(number INT, rate REAL)");
        var values = string.Join(",", Enumerable.Range(1, 300).Select(i => $"({i},1.0)"));
        await db.ExecuteAsync($"INSERT INTO loans VALUES {values},(301,NULL),(302,2); CREATE INDEX by_rate ON loans(rate)");
        rowScans = 0;
        var result = (await db.ExecuteAsync("SELECT number FROM loans WHERE 1.00 = RATE ORDER BY number"))[0];
        Assert.Equal(Enumerable.Range(1, 300).Select(i => i.ToString()), result.Rows.Select(row => row[0].Display));
        Assert.Empty((await db.ExecuteAsync("SELECT number FROM loans WHERE rate=3"))[0].Rows);
        Assert.Equal(0, rowScans);
        Assert.Empty((await db.ExecuteAsync("SELECT number FROM loans WHERE rate=NULL"))[0].Rows);
        Assert.Equal("301", Assert.Single((await db.ExecuteAsync("SELECT number FROM loans WHERE rate IS NULL"))[0].Rows)[0].Display);
    }

    [Fact]
    public async Task Index_ddl_rollback_and_table_recreation_do_not_leave_stale_entries()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t (id INT, name TEXT); INSERT INTO t VALUES (1,'old'); BEGIN; CREATE INDEX by_name ON t(name); ROLLBACK");
        await db.ExecuteAsync("CREATE INDEX by_name ON t(name); BEGIN; DROP INDEX by_name ON t; ROLLBACK");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE INDEX by_name ON t(name)"));
        await db.ExecuteAsync("DROP INDEX by_name ON t; UPDATE t SET name='new'; CREATE INDEX by_name ON t(name)");
        Assert.Empty((await db.ExecuteAsync("SELECT id FROM t WHERE name='old'"))[0].Rows);
        Assert.Single((await db.ExecuteAsync("SELECT id FROM t WHERE name='new'"))[0].Rows);
        await db.ExecuteAsync("DROP TABLE t; CREATE TABLE t (id INT, name TEXT); CREATE INDEX by_name ON t(name)");
        Assert.Empty((await db.ExecuteAsync("SELECT id FROM t WHERE name='new'"))[0].Rows);
    }

    [Fact]
    public async Task Interrupted_index_creation_rolls_back_both_metadata_and_entries()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t(id INT, v TEXT); INSERT INTO t VALUES (1,'cat'),(2,'dog')");
        int writes = 0;
        var interrupted = new SqlDatabase(request => request["op"]!.ToString() == "put" && ++writes == 2
            ? throw new IOException("injected index write failure") : store.RequestAsync(request), () => store.InTransaction);
        await Assert.ThrowsAsync<IOException>(() => interrupted.ExecuteAsync("CREATE INDEX by_v ON t(v)"));
        Assert.False(store.InTransaction);
        await db.ExecuteAsync("UPDATE t SET v='dog' WHERE id=1; CREATE INDEX by_v ON t(v)");
        Assert.Empty((await db.ExecuteAsync("SELECT id FROM t WHERE v='cat'"))[0].Rows);
        Assert.Equal(2, (await db.ExecuteAsync("SELECT id FROM t WHERE v='dog'"))[0].Rows.Count);
    }
}
