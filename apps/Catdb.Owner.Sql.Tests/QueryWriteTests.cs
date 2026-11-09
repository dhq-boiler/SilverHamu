using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class QueryWriteTests
{
    private static SqlDatabase Database()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Insert_select_reads_a_snapshot_and_validates_the_whole_result()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE t (id INT PRIMARY KEY, tag TEXT DEFAULT 'new');
            INSERT INTO t VALUES (1,'old'),(2,'old');
            INSERT INTO t (id) SELECT id+2 FROM t;
            BEGIN;
            INSERT INTO t VALUES (9,'pending');
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t (id) SELECT id+5 FROM t"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t SELECT id FROM t WHERE id=0"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t (id) SELECT tag FROM t WHERE id=0"));
        var page = (await db.ExecuteAsync("SELECT id,tag FROM t ORDER BY id")).Single();
        Assert.Equal(["1 old", "2 old", "3 new", "4 new", "9 pending"], page.Rows.Select(r => string.Join(' ', r.Select(v => v.Display))));
        await db.ExecuteAsync("ROLLBACK");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM t WHERE id=9")).Single().Rows);
    }

    [Fact]
    public async Task Compound_write_sources_never_commit_an_implicit_first_page()
    {
        var db = Database();
        var seed = string.Join(",", Enumerable.Repeat("(1)", 64));
        await db.ExecuteAsync($"CREATE TABLE t(k INT); INSERT INTO t VALUES {seed}; CREATE TABLE dest(k INT)");
        const string source = "SELECT a.k FROM t a CROSS JOIN t b UNION ALL SELECT 2";
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE cut AS " + source));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO dest " + source));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"MERGE INTO dest USING ({source}) s ON dest.k=s.k WHEN NOT MATCHED THEN INSERT VALUES(s.k)"));
        Assert.DoesNotContain(await db.GetTablesAsync(), table => table.Name == "cut");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM dest"))[0].Rows);
        await db.ExecuteAsync("INSERT INTO dest " + source + " FETCH FIRST 1 ROW ONLY");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM dest"))[0].Rows);
    }

    [Fact]
    public async Task Ctas_preserves_empty_input_schema_and_rolls_back_catalog_and_rows_together()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE t (id INT PRIMARY KEY, label TEXT);
            INSERT INTO t VALUES (1,'one'),(2,'two');
            CREATE TABLE empty_copy (n, name) AS SELECT id, label FROM t WHERE id < 0;
            INSERT INTO empty_copy VALUES (3,'three');
            BEGIN;
            CREATE TABLE copy AS SELECT id, label AS name FROM t;
            """);
        Assert.Equal(["INTEGER", "TEXT"], (await db.GetTablesAsync()).Single(t => t.Name == "empty_copy").Columns.Select(c => c.Type));
        Assert.Equal("two", (await db.ExecuteAsync("SELECT name FROM copy WHERE id=2")).Single().Rows.Single()[0].Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE bad AS SELECT id, id FROM t"));
        Assert.DoesNotContain(await db.GetTablesAsync(), t => t.Name == "bad");
        await db.ExecuteAsync("ROLLBACK");
        Assert.DoesNotContain(await db.GetTablesAsync(), t => t.Name == "copy");
    }

    [Fact]
    public async Task Update_expressions_use_original_row_for_all_assignments_and_indexes()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE t (id INT PRIMARY KEY, a INT, b INT);
            CREATE INDEX ia ON t (a);
            INSERT INTO t VALUES (1,2,3),(2,NULL,8);
            UPDATE t SET a = b * 2, b = t.a + 1;
            """);
        var result = (await db.ExecuteAsync("SELECT id,a,b FROM t ORDER BY id")).Single();
        Assert.Equal(["1 6 3", "2 16 NULL"], result.Rows.Select(r => string.Join(' ', r.Select(v => v.Display))));
        Assert.Single((await db.ExecuteAsync("SELECT id FROM t WHERE a=6")).Single().Rows);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE t SET id=1 WHERE id=2"));
        Assert.Single((await db.ExecuteAsync("SELECT id FROM t WHERE id=2")).Single().Rows);
    }

    [Fact]
    public async Task Merge_uses_first_matching_clause_and_keeps_indexes_consistent()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE stock (id INT PRIMARY KEY, qty INT CHECK (qty >= 0));
            CREATE INDEX by_qty ON stock (qty);
            INSERT INTO stock VALUES (1,10),(2,5);
            CREATE TABLE incoming (id INT, delta INT);
            INSERT INTO incoming VALUES (1,3),(2,0),(3,7),(4,-1);
            MERGE INTO stock AS t USING incoming AS s ON t.id=s.id
              WHEN MATCHED AND s.delta=0 THEN DELETE
              WHEN MATCHED THEN UPDATE SET qty=t.qty+s.delta
              WHEN NOT MATCHED AND s.delta>0 THEN INSERT (id,qty) VALUES (s.id,s.delta);
            """);
        var result = (await db.ExecuteAsync("SELECT * FROM stock ORDER BY id")).Single();
        Assert.Equal(["1 13", "3 7"], result.Rows.Select(r => string.Join(' ', r.Select(v => v.Display))));
        Assert.Single((await db.ExecuteAsync("SELECT id FROM stock WHERE qty=13")).Single().Rows);
        Assert.Empty((await db.ExecuteAsync("SELECT id FROM stock WHERE id=2")).Single().Rows);
    }

    [Fact]
    public async Task Merge_cardinality_and_constraint_errors_are_atomic_in_an_open_transaction()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE t (id INT PRIMARY KEY, qty INT NOT NULL); BEGIN; INSERT INTO t VALUES (1,10)");
        const string prefix = "MERGE INTO t USING (VALUES ";
        const string suffix = """
            ) AS s(id,qty) ON s.id=t.id
            WHEN MATCHED THEN UPDATE SET qty=s.qty
            WHEN NOT MATCHED THEN INSERT VALUES (s.id,s.qty)
            """;
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(prefix + "(1,20),(1,30)" + suffix));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(prefix + "(1,20),(2,NULL)" + suffix));
        Assert.Equal("10", (await db.ExecuteAsync("SELECT qty FROM t")).Single().Rows.Single()[0].Display);
        await db.ExecuteAsync("""
            MERGE INTO t USING (SELECT id+1 AS id, qty*2 AS qty FROM t) AS s ON s.id=t.id
              WHEN NOT MATCHED THEN INSERT VALUES (s.id,s.qty);
            COMMIT;
            """);
        Assert.Equal("20", (await db.ExecuteAsync("SELECT qty FROM t WHERE id=2")).Single().Rows.Single()[0].Display);
    }
}
