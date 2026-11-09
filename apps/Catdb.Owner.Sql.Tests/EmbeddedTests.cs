using System.Text;
using System.Text.Json.Nodes;
using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class EmbeddedTests : IDisposable
{
    private readonly string directory = Path.Combine(Path.GetTempPath(), "catdb-owner-embedded-" + Guid.NewGuid().ToString("N"));
    public EmbeddedTests() => Directory.CreateDirectory(directory);
    private string DbPath(string name = "database.ndb") => Path.Combine(directory, name);
    private static SqlDatabase Sql(EmbeddedCatdbStore store) => new(store.RequestAsync, () => store.InTransaction);

    [Fact]
    public async Task Reopening_preserves_named_types_and_binds_casts_in_saved_views_and_checks()
    {
        var path = DbPath();
        using (var db = await CatdbSqlConnection.OpenAsync(path, create: true))
        {
            await db.ExecuteAsync("""
                CREATE TYPE TicketId AS DISTINCT INTEGER;
                CREATE TABLE tickets (id TicketId PRIMARY KEY, CHECK (id <> CAST(0 AS TicketId)));
                INSERT INTO tickets VALUES (7);
                CREATE VIEW visible AS SELECT id FROM tickets WHERE id = CAST(7 AS TicketId)
                """);
        }
        using var reopened = await CatdbSqlConnection.OpenAsync(path);
        var result = Assert.Single(await reopened.ExecuteAsync("SELECT id FROM visible"));
        Assert.Equal("TicketId", Assert.Single(result.ColumnTypes!));
        Assert.Equal("TicketId", Assert.Single(Assert.Single(result.Rows)).TypeName);
        await Assert.ThrowsAsync<SqlException>(() => reopened.ExecuteAsync("INSERT INTO tickets VALUES (0)"));
    }

    [Fact]
    public async Task Savepoints_restore_rows_indexes_and_catalog_across_SQL_calls()
    {
        using var db = await CatdbSqlConnection.OpenAsync(DbPath(), create: true);
        await db.ExecuteAsync("CREATE TABLE t(id INT PRIMARY KEY); START TRANSACTION; INSERT INTO t VALUES (1); SAVEPOINT Mark");
        await db.ExecuteAsync("INSERT INTO t VALUES (2); SAVEPOINT [途中]; DROP TABLE t");
        await db.ExecuteAsync("ROLLBACK TO SAVEPOINT mArk");
        Assert.True(db.InTransaction);
        Assert.Equal("1", Assert.Single((await db.ExecuteAsync("SELECT id FROM t WHERE id=1"))[0].Rows)[0].Display);
        Assert.Empty((await db.ExecuteAsync("SELECT id FROM t WHERE id=2"))[0].Rows);
        await Assert.ThrowsAsync<CatdbRequestException>(() => db.ExecuteAsync("RELEASE SAVEPOINT [途中]"));
        await db.ExecuteAsync("DELETE FROM t; ROLLBACK TO mark; RELEASE SAVEPOINT mark; COMMIT");
        Assert.False(db.InTransaction);
        Assert.Single((await db.ExecuteAsync("SELECT id FROM t"))[0].Rows);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SAVEPOINT outside"));
        await db.ExecuteAsync("START TRANSACTION");
        await Assert.ThrowsAsync<CatdbRequestException>(() => db.ExecuteAsync("ROLLBACK TO mark"));
        await db.ExecuteAsync("ROLLBACK");
    }

    [Fact]
    public async Task Core_session_and_owner_share_the_same_committed_schema_rows_and_indexes()
    {
        var path = DbPath();
        using (var core = await CatdbSqlConnection.OpenAsync(path, create: true))
        {
            await core.ExecuteAsync("CREATE TABLE books (id INT PRIMARY KEY, title TEXT); INSERT INTO books VALUES (1,'initial'); CREATE INDEX by_title ON books(title);");
        }
        using (var owner = await EmbeddedCatdbStore.OpenAsync(path))
        {
            var sql = Sql(owner);
            var initial = (await sql.ExecuteAsync("SELECT title FROM books WHERE id=1"))[0];
            Assert.Equal("initial", Assert.Single(Assert.Single(initial.Rows)).Display);
            await sql.ExecuteAsync("UPDATE books SET title='updated' WHERE id=1; CREATE VIEW published AS SELECT title FROM books;");
        }
        using var reopened = await CatdbSqlConnection.OpenAsync(path);
        var results = await reopened.ExecuteAsync("SELECT id FROM books WHERE title='updated'; SELECT * FROM published");
        Assert.Equal("1", Assert.Single(Assert.Single(results[0].Rows)).Display);
        Assert.Equal("updated", Assert.Single(Assert.Single(results[1].Rows)).Display);
    }

    [Fact]
    public async Task Owner_snapshot_exports_only_committed_values_in_range_and_never_replaces_files()
    {
        using var store = await EmbeddedCatdbStore.OpenAsync(DbPath(), create: true);
        await store.RequestAsync(new() { ["op"] = "begin" });
        foreach (var key in new[] { "01", "02", "03" })
            await store.RequestAsync(new() { ["op"] = "put", ["key"] = key, ["value"] = key + "AB" });
        await store.RequestAsync(new() { ["op"] = "commit" });
        await store.RequestAsync(new() { ["op"] = "begin" });
        await store.RequestAsync(new() { ["op"] = "delete", ["key"] = "02" });
        var path = DbPath("committed.snapshot");
        var generation = Guid.NewGuid();
        var info = await store.ExportSnapshotAsync(path, generation, [2], [3]);
        Assert.Equal(1UL, info.Records);
        Assert.True(store.InTransaction);
        await Assert.ThrowsAsync<IOException>(() => store.ExportSnapshotAsync(path, Guid.NewGuid(), [], null));
        using var snapshot = new NewDbms.NewDbmsSnapshot(path, generation.ToByteArray());
        Assert.Equal(new byte[] { 2, 0xab }, Assert.Single(snapshot.ReadPage(0, 2)));
    }

    [Fact]
    public async Task Owner_online_backup_restores_the_committed_state_to_a_new_database()
    {
        var path = DbPath();
        var backup = DbPath("snapshot.backup");
        var target = DbPath("restored.ndb");
        using var owner = await EmbeddedCatdbStore.OpenAsync(path, create: true);
        var sql = Sql(owner);
        await sql.ExecuteAsync("CREATE TABLE t(id INT PRIMARY KEY); INSERT INTO t VALUES (1); BEGIN; INSERT INTO t VALUES (2)");
        await owner.BackupAsync(backup);
        Assert.True(owner.InTransaction);
        await sql.ExecuteAsync("COMMIT");
        await EmbeddedCatdbStore.RestoreAsync(backup, target);
        using var restored = await CatdbSqlConnection.OpenAsync(target);
        var result = (await restored.ExecuteAsync("SELECT id FROM t"))[0];
        Assert.Equal("1", Assert.Single(Assert.Single(result.Rows)).Display);
        Assert.Equal(2, (await sql.ExecuteAsync("SELECT * FROM t"))[0].Rows.Count);
    }

    [Fact]
    public async Task EQ_native_SQL_persists_schema_types_and_committed_changes_across_reopen()
    {
        var path = DbPath("組み込み猫.ndb");
        using (var store = await EmbeddedCatdbStore.OpenAsync(path, create: true))
        {
            await Sql(store).ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, image BLOB, value REAL); INSERT INTO t VALUES (1,'猫🐈',X'00FF',12.5),(2,'remove',NULL,NULL); UPDATE t SET value = 20 WHERE id = 1; DELETE FROM t WHERE id = 2;");
            Assert.False(store.InTransaction);
        }
        using var reopened = await EmbeddedCatdbStore.OpenAsync(path);
        var rows = (await Sql(reopened).ExecuteAsync("SELECT * FROM t;"))[0].Rows;
        Assert.Equal(new[] { "1", "猫🐈", "0x00FF", "20" }, Assert.Single(rows).Select(x => x.Display));
        Assert.Single(await Sql(reopened).GetTablesAsync());
    }

    [Fact]
    public async Task Join_reads_both_sources_in_the_current_native_transaction()
    {
        var path = DbPath();
        using var store = await EmbeddedCatdbStore.OpenAsync(path, create: true);
        var db = Sql(store);
        await db.ExecuteAsync("CREATE TABLE books(id INT PRIMARY KEY, author_id INT); CREATE TABLE authors(id INT PRIMARY KEY, name TEXT); INSERT INTO books VALUES (1,10); INSERT INTO authors VALUES (10,'old');");
        const string query = "SELECT b.id, a.name FROM books b JOIN authors a ON b.author_id=a.id ORDER BY b.id";
        await db.ExecuteAsync("BEGIN; UPDATE authors SET name='updated' WHERE id=10; INSERT INTO books VALUES (2,10)");
        Assert.Equal(new[] { "updated", "updated" }, (await db.ExecuteAsync(query))[0].Rows.Select(r => r[1].Display));
        await db.ExecuteAsync("ROLLBACK");
        Assert.Equal("old", Assert.Single((await db.ExecuteAsync(query))[0].Rows)[1].Display);
    }

    [Fact]
    public async Task Secondary_index_tracks_committed_updates_deletes_and_nulls_across_reopen()
    {
        var path = DbPath();
        using (var store = await EmbeddedCatdbStore.OpenAsync(path, create: true))
        {
            var db = Sql(store);
            await db.ExecuteAsync("CREATE TABLE books(id INT PRIMARY KEY, shelf TEXT); INSERT INTO books VALUES (1,'A'),(2,'A'),(3,NULL),(4,'B'); CREATE INDEX by_shelf ON books(shelf)");
            await db.ExecuteAsync("BEGIN; UPDATE books SET shelf='B' WHERE id=1; UPDATE books SET shelf='B' WHERE id=3; UPDATE books SET shelf=NULL WHERE id=2; DELETE FROM books WHERE id=4; INSERT INTO books VALUES (5,'B'); COMMIT");
            await db.ExecuteAsync("BEGIN; UPDATE books SET shelf='A' WHERE shelf='B'; DELETE FROM books WHERE id=2");
            Assert.Equal(3, (await db.ExecuteAsync("SELECT id FROM books WHERE shelf='A'"))[0].Rows.Count);
        }
        using var reopened = await EmbeddedCatdbStore.OpenAsync(path);
        var restored = Sql(reopened);
        var indexed = (await restored.ExecuteAsync("SELECT id FROM books WHERE shelf='B' ORDER BY id"))[0];
        var scanned = (await restored.ExecuteAsync("SELECT id FROM books WHERE shelf='B' AND TRUE ORDER BY id"))[0];
        Assert.Equal(new[] { "1", "3", "5" }, indexed.Rows.Select(row => row[0].Display));
        Assert.Equal(scanned.Rows.Select(row => row[0].Display), indexed.Rows.Select(row => row[0].Display));
        Assert.Empty((await restored.ExecuteAsync("SELECT id FROM books WHERE shelf='A'"))[0].Rows);
        Assert.Equal("2", Assert.Single((await restored.ExecuteAsync("SELECT id FROM books WHERE shelf IS NULL"))[0].Rows)[0].Display);
    }

    [Fact]
    public async Task Primary_key_lookup_agrees_with_scan_after_native_reopen()
    {
        var path = DbPath();
        using (var store = await EmbeddedCatdbStore.OpenAsync(path, create: true))
        {
            await Sql(store).ExecuteAsync("CREATE TABLE t(id TEXT PRIMARY KEY, v INT); INSERT INTO t VALUES ('猫',1),('犬',2); UPDATE t SET id='鳥' WHERE id='猫'; DELETE FROM t WHERE id='犬'; BEGIN; UPDATE t SET id='未確定' WHERE id='鳥'");
        }
        using var reopened = await EmbeddedCatdbStore.OpenAsync(path);
        var db = Sql(reopened);
        foreach (var key in new[] { "猫", "犬", "鳥", "未確定" })
        {
            var predicate = $"id='{key}'";
            var indexed = (await db.ExecuteAsync($"SELECT v FROM t WHERE {predicate}"))[0];
            var scanned = (await db.ExecuteAsync($"SELECT v FROM t WHERE {predicate} AND TRUE"))[0];
            Assert.Equal(scanned.Rows.Select(r => r[0].Display), indexed.Rows.Select(r => r[0].Display));
            Assert.Equal(key == "鳥" ? 1 : 0, indexed.Rows.Count);
        }
    }

    [Fact]
    public async Task View_reopens_with_only_projected_columns_and_matching_rows()
    {
        var path = DbPath();
        using (var store = await EmbeddedCatdbStore.OpenAsync(path, create: true))
            await Sql(store).ExecuteAsync("CREATE TABLE books(id INT, secret TEXT, visible BOOLEAN); CREATE VIEW shelf AS SELECT id AS book FROM books WHERE visible = TRUE; INSERT INTO books VALUES (1,'secret',TRUE),(2,'hidden',FALSE);");

        using var reopened = await EmbeddedCatdbStore.OpenAsync(path);
        var db = Sql(reopened);
        var result = (await db.ExecuteAsync("SELECT * FROM SHELF"))[0];
        Assert.Equal("book", Assert.Single(result.Columns));
        Assert.Equal("1", Assert.Single(Assert.Single(result.Rows)).Display);
        Assert.Single((await db.ExecuteAsync("SHOW VIEWS"))[0].Rows);
        var published = await db.QueryPublishedViewAsync("SELECT * FROM shelf", ["SHELF"]);
        Assert.Equal("1", Assert.Single(Assert.Single(published.Rows)).Display);
        foreach (var sql in new[] { "SELECT secret FROM books", "DELETE FROM shelf", "SHOW TABLES", "SELECT * FROM shelf; SELECT * FROM books", "SELECT secret FROM shelf" })
            await Assert.ThrowsAsync<SqlException>(() => db.QueryPublishedViewAsync(sql, ["shelf"]));
        await Assert.ThrowsAsync<SqlException>(() => db.QueryPublishedViewAsync("SELECT * FROM books", ["books"]));
        foreach (var sql in new[] { "SELECT secret FROM shelf", "SELECT book FROM shelf WHERE secret IS NULL",
                     "UPDATE shelf SET secret='private'", "INSERT INTO shelf(secret) VALUES ('private')", "DELETE FROM shelf WHERE visible=FALSE",
                     "DROP TABLE shelf", "DROP TABLE books", "CREATE TABLE shelf(id INT)", "CREATE VIEW books AS SELECT id FROM books",
                     "CREATE VIEW invalid AS SELECT id,id FROM books", "CREATE VIEW invalid AS SELECT missing FROM books" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(sql));

        await db.ExecuteAsync("BEGIN; UPDATE shelf SET book=3; INSERT INTO shelf VALUES (4); DELETE FROM shelf WHERE book=3;");
        var remaining = (await db.ExecuteAsync("SELECT id FROM books ORDER BY id")).Single().Rows;
        Assert.Equal(["2", "4"], remaining.Select(row => row[0].Display).ToArray());
        await db.ExecuteAsync("ROLLBACK");
        await db.ExecuteAsync("BEGIN; DROP VIEW shelf; ROLLBACK; UPDATE books SET visible=TRUE WHERE id=2;");
        Assert.Equal("2", (await db.ExecuteAsync("SELECT COUNT(*) FROM shelf"))[0].Rows[0][0].Display);
        await db.ExecuteAsync("DROP VIEW shelf; DROP TABLE books;");
        Assert.Empty(await db.GetViewsAsync());
    }

    [Fact]
    public async Task ST_dispose_discards_uncommitted_rows_and_releases_ownership()
    {
        var path = DbPath();
        using (var store = await EmbeddedCatdbStore.OpenAsync(path, create: true))
        {
            await Sql(store).ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY); BEGIN; INSERT INTO t VALUES (1);");
            Assert.True(store.InTransaction);
        }
        using var reopened = await EmbeddedCatdbStore.OpenAsync(path);
        Assert.Empty((await Sql(reopened).ExecuteAsync("SELECT * FROM t;"))[0].Rows);
        await Sql(reopened).ExecuteAsync("BEGIN; INSERT INTO t VALUES (2); ROLLBACK;");
        Assert.Empty((await Sql(reopened).ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task ST_a_second_owner_is_rejected_without_disrupting_the_first_owner()
    {
        var path = DbPath();
        using var owner = await EmbeddedCatdbStore.OpenAsync(path, create: true);
        var error = await Assert.ThrowsAsync<IOException>(() => EmbeddedCatdbStore.OpenAsync(path));
        Assert.Contains("使用中", error.Message);
        await Sql(owner).ExecuteAsync("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1);");
        Assert.Single((await Sql(owner).ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task ER_open_does_not_create_a_missing_file_and_create_never_overwrites()
    {
        var path = DbPath();
        await Assert.ThrowsAsync<FileNotFoundException>(() => EmbeddedCatdbStore.OpenAsync(path));
        Assert.False(File.Exists(path));
        await File.WriteAllBytesAsync(path, Encoding.UTF8.GetBytes("original file"));
        await Assert.ThrowsAsync<IOException>(() => EmbeddedCatdbStore.OpenAsync(path, create: true));
        Assert.Equal("original file", await File.ReadAllTextAsync(path));
        await Assert.ThrowsAsync<IOException>(() => EmbeddedCatdbStore.OpenAsync(path));
        Assert.Equal("original file", await File.ReadAllTextAsync(path));
    }

    [Fact]
    public async Task EQ_native_scan_continues_over_multiple_pages_and_preserves_null_and_empty()
    {
        using var store = await EmbeddedCatdbStore.OpenAsync(DbPath(), create: true);
        await Sql(store).ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES " + string.Join(',', Enumerable.Range(0, 300).Select(i => $"({i},{(i == 0 ? "NULL" : "''")})")) + ";");
        var result = (await Sql(store).ExecuteAsync("SELECT * FROM t ORDER BY id;"))[0];
        Assert.Equal(300, result.Rows.Count);
        Assert.True(result.Rows[0][1].IsNull);
        Assert.Equal("", result.Rows[1][1].Text);
        Assert.Equal("299", result.Rows[^1][0].Text);
    }

    [Fact]
    public async Task ST_duplicate_begin_keeps_transaction_usable_until_rollback()
    {
        var path = DbPath();
        using var store = await EmbeddedCatdbStore.OpenAsync(path, create: true);
        await Sql(store).ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY); BEGIN;");
        await Assert.ThrowsAsync<CatdbRequestException>(() => store.RequestAsync(new JsonObject { ["op"] = "begin" }));
        Assert.True(store.InTransaction);
        await Sql(store).ExecuteAsync("ROLLBACK; INSERT INTO t VALUES (1);");
        Assert.False(store.InTransaction);
        Assert.Single((await Sql(store).ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task ER_requests_after_dispose_fail_and_dispose_is_idempotent()
    {
        var store = await EmbeddedCatdbStore.OpenAsync(DbPath(), create: true);
        store.Dispose(); store.Dispose();
        await Assert.ThrowsAsync<ObjectDisposedException>(() => store.RequestAsync(new JsonObject { ["op"] = "begin" }));
    }

    public void Dispose() => Directory.Delete(directory, recursive: true);
}
