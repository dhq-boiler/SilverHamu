using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class DmlFeatureTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Composite_upsert_returns_only_changed_rows_and_uses_both_row_images()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t(a INT,b INT,v INT,PRIMARY KEY(a,b)); INSERT INTO t VALUES (1,2,4)");
        var result = (await db.ExecuteAsync("""
            INSERT INTO t VALUES (1,2,3),(1,3,8)
            ON CONFLICT(b,a) DO UPDATE SET v=t.v+excluded.v WHERE excluded.v>0 RETURNING a,b,v*2 AS doubled
            """))[0];
        Assert.Equal(new[] { "a", "b", "doubled" }, result.Columns);
        Assert.Equal(new[] { "14", "16" }, result.Rows.Select(r => r[2].Text));
        var skipped = (await db.ExecuteAsync("""
            INSERT INTO t VALUES (1,2,-1) ON CONFLICT(a,b) DO UPDATE SET v=excluded.v WHERE excluded.v>0 RETURNING *
            """))[0];
        Assert.Empty(skipped.Rows);
        Assert.Equal(new[] { "INTEGER", "INTEGER", "INTEGER" }, skipped.ColumnTypes);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES (1,2,1),(1,2,1) ON CONFLICT(a,b) DO UPDATE SET v=v+1"));
        Assert.Equal("7", (await db.ExecuteAsync("DELETE FROM t WHERE b=2 RETURNING v"))[0].Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Upsert_uses_unique_indexes_and_preserves_multiple_null_keys()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t(k INT,n INT); CREATE UNIQUE INDEX u ON t(k); INSERT INTO t VALUES(1,2),(NULL,5)");
        var result = (await db.ExecuteAsync("INSERT INTO t VALUES(1,4),(NULL,8) ON CONFLICT(k) DO UPDATE SET n=excluded.n RETURNING n"))[0];
        Assert.Equal(new[] { "4", "8" }, result.Rows.Select(r => r[0].Text));
        Assert.Empty((await db.ExecuteAsync("INSERT INTO t VALUES(1,99) ON CONFLICT DO NOTHING RETURNING n"))[0].Rows);
        Assert.Equal("5", (await db.ExecuteAsync("UPDATE t SET n=n+1 WHERE k=1 RETURNING t.*"))[0].Rows.Single()[1].Text);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES(0,0) ON CONFLICT(n) DO UPDATE SET k=1"));
    }

    [Fact]
    public async Task Truncate_respects_external_references_and_rolls_back_rows_and_indexes()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE p(k INT PRIMARY KEY); CREATE TABLE c(p INT REFERENCES p(k)); INSERT INTO p VALUES(1); INSERT INTO c VALUES(1)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("TRUNCATE TABLE p"));
        await db.ExecuteAsync("DROP TABLE c; BEGIN; TRUNCATE TABLE p; INSERT INTO p VALUES(2); ROLLBACK");
        Assert.Equal("1", (await db.ExecuteAsync("SELECT k FROM p WHERE k=1"))[0].Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Trigger_effects_reopen_and_follow_transaction_rollback()
    {
        var directory = Directory.CreateTempSubdirectory("silverhamu-trigger-");
        try
        {
            var path = Path.Combine(directory.FullName, "db.ndb");
            using (var store = await EmbeddedSilverHamuStore.OpenAsync(path, create: true))
                await new SqlDatabase(store.RequestAsync, () => store.InTransaction).ExecuteAsync("""
                    CREATE TABLE t(id INT PRIMARY KEY,v INT); CREATE TABLE ledger(kind TEXT,id INT,v INT);
                    CREATE TRIGGER added AFTER INSERT ON t FOR EACH ROW BEGIN INSERT INTO ledger VALUES ('add',NEW.id,NEW.v); END;
                    CREATE TRIGGER changed AFTER UPDATE ON t WHEN OLD.v<>NEW.v BEGIN INSERT INTO ledger VALUES ('delta',NEW.id,NEW.v-OLD.v); END;
                    CREATE TRIGGER removed AFTER DELETE ON t BEGIN INSERT INTO ledger VALUES ('remove',OLD.id,OLD.v); END;
                    """);
            using var reopened = await EmbeddedSilverHamuStore.OpenAsync(path);
            var roundtrip = new SqlDatabase(reopened.RequestAsync, () => reopened.InTransaction);
            await roundtrip.ExecuteAsync("""
                INSERT INTO t VALUES(1,4); INSERT INTO t VALUES(1,6) ON CONFLICT(id) DO UPDATE SET v=excluded.v;
                UPDATE t SET v=6; BEGIN; DELETE FROM t; ROLLBACK;
                """);
            var audit = (await roundtrip.ExecuteAsync("SELECT kind,v FROM ledger ORDER BY kind"))[0];
            Assert.Equal(new[] { "add:4", "delta:2" }, audit.Rows.Select(r => r[0].Text + ":" + r[1].Text));
            await roundtrip.ExecuteAsync("DELETE FROM t; DROP TRIGGER removed");
            Assert.Equal("6", (await roundtrip.ExecuteAsync("SELECT v FROM ledger WHERE kind='remove'"))[0].Rows.Single()[0].Text);
        }
        finally { directory.Delete(true); }
    }

    [Fact]
    public async Task Failing_trigger_rolls_back_its_statement_but_preserves_prior_explicit_work()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE t(k INT PRIMARY KEY); CREATE TABLE audit(k INT PRIMARY KEY);
            CREATE TRIGGER log AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(NEW.k); END;
            BEGIN; INSERT INTO audit VALUES(2); INSERT INTO t VALUES(3);
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES(1),(2) RETURNING *"));
        await db.ExecuteAsync("COMMIT");
        Assert.Equal(new[] { "2", "3" }, (await db.ExecuteAsync("SELECT k FROM audit ORDER BY k"))[0].Rows.Select(r => r[0].Text));
        Assert.Equal("3", (await db.ExecuteAsync("SELECT k FROM t"))[0].Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Recursive_triggers_stop_at_the_depth_limit_without_partial_effects()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t(k INT PRIMARY KEY)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(
            "CREATE TRIGGER bad AFTER INSERT ON t BEGIN INSERT INTO t VALUES(OLD.k); END"));
        await db.ExecuteAsync("CREATE TRIGGER more AFTER INSERT ON t BEGIN INSERT INTO t VALUES(NEW.k+1); END");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES(1)"));
        Assert.Empty((await db.ExecuteAsync("SELECT k FROM t"))[0].Rows);
    }

    [Fact]
    public async Task Trusted_trigger_actions_follow_foreign_key_cascades_but_cannot_be_rebound_to_another_table()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE parents(k INT PRIMARY KEY); CREATE TABLE children(parent INT REFERENCES parents(k) ON DELETE CASCADE);
            CREATE TABLE audit(k INT);
            CREATE TRIGGER erased AFTER DELETE ON children BEGIN INSERT INTO audit VALUES(OLD.parent); END;
            INSERT INTO parents VALUES(7); INSERT INTO children VALUES(7);
            GRANT DELETE ON parents TO writer;
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DROP TABLE audit"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE children RENAME COLUMN parent TO lost"));
        await db.ExecuteRoleAsync("DELETE FROM parents WHERE k=7", "writer");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("INSERT INTO audit VALUES(8)", "writer"));
        Assert.Equal("7", Assert.Single(Assert.Single((await db.ExecuteAsync("SELECT k FROM audit"))[0].Rows)).Text);
    }

    [Fact]
    public async Task Role_upserts_require_update_and_returning_respects_view_visibility()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE t(id INT PRIMARY KEY,secret INT DEFAULT 17); CREATE VIEW published AS SELECT id AS n FROM t WHERE id>0;
            GRANT INSERT ON t TO limited; GRANT INSERT,UPDATE ON published TO limited;
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("INSERT INTO t VALUES(1,2) ON CONFLICT(id) DO UPDATE SET secret=9", "limited"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("INSERT INTO published VALUES(1) RETURNING *", "limited"));
        await db.ExecuteAsync("GRANT SELECT ON published TO limited");
        var result = await db.ExecuteRoleAsync("INSERT INTO published VALUES(1) RETURNING *", "limited");
        Assert.Equal("n", Assert.Single(result.Columns));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("UPDATE published SET n=2 RETURNING secret", "limited"));
        Assert.Empty((await db.ExecuteRoleAsync("UPDATE published SET n=-1 RETURNING n", "limited")).Rows);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteRoleAsync("TRUNCATE TABLE t", "limited"));
    }
}
