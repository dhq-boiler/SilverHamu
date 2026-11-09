using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class IndexAndReferenceActionTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task A_partial_unique_index_only_limits_and_returns_matching_non_null_keys()
    {
        var directory = Directory.CreateTempSubdirectory("catdb-partial-");
        var path = Path.Combine(directory.FullName, "index.ndb");
        try
        {
            using (var db = await CatdbSqlConnection.OpenAsync(path, true))
            {
                await db.ExecuteAsync("""
                    CREATE TABLE items(id INTEGER PRIMARY KEY, code TEXT, live BOOLEAN);
                    INSERT INTO items VALUES(1,'a',TRUE),(2,'a',FALSE),(3,NULL,TRUE),(4,NULL,TRUE);
                    CREATE UNIQUE INDEX live_code ON items(code) WHERE live=TRUE
                    """);
            }
            using var db2 = await CatdbSqlConnection.OpenAsync(path);
            const string filtered = "SELECT id FROM items WHERE code='a' AND live=TRUE";
            Assert.Contains((await db2.ExecuteAsync("EXPLAIN " + filtered))[0].Rows, r => r[1].Text == "INDEX RANGE SCAN");
            Assert.DoesNotContain((await db2.ExecuteAsync("EXPLAIN SELECT id FROM items WHERE code='a'"))[0].Rows, r => r[1].Text == "INDEX RANGE SCAN");
            Assert.Equal(2, (await db2.ExecuteAsync("SELECT id FROM items WHERE code='a' OR live=FALSE"))[0].Rows.Count);
            await Assert.ThrowsAsync<SqlException>(() => db2.ExecuteAsync("UPDATE items SET live=TRUE WHERE id=2"));
            await Assert.ThrowsAsync<SqlException>(() => db2.ExecuteAsync("INSERT INTO items VALUES (5,'a',TRUE)"));
            await Assert.ThrowsAsync<SqlException>(() => db2.ExecuteAsync("ALTER TABLE items DROP COLUMN live"));
            await db2.ExecuteAsync("UPDATE items SET live=NULL WHERE id=1; UPDATE items SET live=TRUE WHERE id=2; REINDEX items");
            Assert.Equal("2", Assert.Single((await db2.ExecuteAsync(filtered))[0].Rows)[0].Text);
            await db2.ExecuteAsync("CREATE INDEX nonnull_code ON items(code) WHERE code IS NOT NULL");
            var plan = Assert.Single(await db2.ExecuteAsync("EXPLAIN SELECT id FROM items WHERE code='a'"));
            Assert.Contains(plan.Rows, r => r[1].Text == "INDEX RANGE SCAN" && r[3].Text == "nonnull_code");
        }
        finally { directory.Delete(recursive: true); }
    }

    [Fact]
    public async Task Unique_index_creation_checks_existing_rows_and_an_fk_prevents_removal_of_its_unique_target()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE p(id INT, code TEXT); INSERT INTO p VALUES (1,'x'),(2,'x')");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE UNIQUE INDEX code_key ON p(code)"));
        await db.ExecuteAsync("DELETE FROM p WHERE id=2; CREATE UNIQUE INDEX code_key ON p(code); CREATE TABLE c(code TEXT REFERENCES p(code))");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DROP INDEX code_key ON p"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO p VALUES (3,'x')"));
        await db.ExecuteAsync("DROP TABLE c; DROP INDEX code_key ON p; INSERT INTO p VALUES (3,'x')");
    }

    [Fact]
    public async Task Composite_set_actions_replace_all_components_and_require_a_valid_default_parent()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE p(a INTEGER,b TEXT, PRIMARY KEY(a,b));
            CREATE TABLE c(a INTEGER DEFAULT 0,b TEXT DEFAULT 'default',
                FOREIGN KEY(a,b) REFERENCES p(a,b) ON UPDATE SET NULL ON DELETE SET DEFAULT);
            INSERT INTO p VALUES (1,'one'),(2,'two');
            INSERT INTO c VALUES (1,'one'),(2,'two');
            UPDATE p SET a=3 WHERE a=1
            """);
        var nulled = Assert.Single((await db.ExecuteAsync("SELECT * FROM c WHERE a IS NULL"))[0].Rows);
        Assert.All(nulled, value => Assert.True(value.IsNull));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM p WHERE a=2"));
        Assert.Single((await db.ExecuteAsync("SELECT a FROM c WHERE a=2"))[0].Rows);
        await db.ExecuteAsync("INSERT INTO p VALUES (0,'default'); DELETE FROM p WHERE a=2");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM c WHERE a=0 AND b='default'"))[0].Rows);
    }

    [Fact]
    public async Task Setting_a_child_key_to_null_propagates_to_its_own_dependents()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE p(k INTEGER PRIMARY KEY);
            CREATE TABLE c(k INTEGER UNIQUE REFERENCES p(k) ON DELETE SET NULL);
            CREATE TABLE g(k INTEGER REFERENCES c(k) ON UPDATE CASCADE);
            INSERT INTO p VALUES(1); INSERT INTO c VALUES(1); INSERT INTO g VALUES(1);
            DELETE FROM p
            """);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM g WHERE k IS NULL"))[0].Rows);
    }

    [Fact]
    public async Task A_failed_set_null_action_keeps_the_parent_and_required_child_key()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE p(k INTEGER PRIMARY KEY);
            CREATE TABLE c(k INTEGER NOT NULL REFERENCES p(k) ON DELETE SET NULL);
            INSERT INTO p VALUES (1); INSERT INTO c VALUES (1); BEGIN
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM p"));
        Assert.Single((await db.ExecuteAsync("SELECT p.k FROM p JOIN c ON p.k=c.k"))[0].Rows);
        await db.ExecuteAsync("COMMIT");
    }
}
