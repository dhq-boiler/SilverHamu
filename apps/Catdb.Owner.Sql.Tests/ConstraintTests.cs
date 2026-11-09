using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class ConstraintTests
{
    private static SqlDatabase Database()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Composite_cascades_follow_row_identity_and_maintain_descendant_indexes()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE parent(site TEXT, id INT, PRIMARY KEY(site,id));
            CREATE TABLE child(site TEXT, id INT, PRIMARY KEY(site,id),
                FOREIGN KEY(site,id) REFERENCES parent(site,id) ON DELETE CASCADE ON UPDATE CASCADE);
            CREATE TABLE grand(child_site TEXT, child_id INT,
                FOREIGN KEY(child_site,child_id) REFERENCES child(site,id) ON UPDATE CASCADE ON DELETE CASCADE);
            CREATE INDEX grand_fk ON grand(child_site,child_id);
            INSERT INTO parent VALUES('A',1),('A',2);
            INSERT INTO child VALUES('A',1);
            INSERT INTO grand VALUES('A',1),('A',NULL);
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO child VALUES('B',1)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE bad(x TEXT,y INT, FOREIGN KEY(x,y) REFERENCES parent(site,site))"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE parent DROP COLUMN id"));
        await db.ExecuteAsync("BEGIN; UPDATE parent SET id=3 WHERE id=1");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM grand WHERE child_site='A' AND child_id=3"))[0].Rows);
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM child WHERE site='A' AND id=1"))[0].Rows);
        await db.ExecuteAsync("ROLLBACK; DELETE FROM parent WHERE id=1");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM child"))[0].Rows);
        Assert.True(Assert.Single((await db.ExecuteAsync("SELECT child_id FROM grand"))[0].Rows)[0].IsNull);
    }

    [Fact]
    public async Task Cascade_across_self_reference_is_atomic_if_another_table_restricts_deletion()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE tree(id INT PRIMARY KEY, parent INT REFERENCES tree(id) ON DELETE CASCADE ON UPDATE CASCADE);
            CREATE TABLE guard(id INT REFERENCES tree(id));
            INSERT INTO tree VALUES(1,1),(2,1),(3,2);
            INSERT INTO guard VALUES(3);
            UPDATE tree SET id=4 WHERE id=1
            """);
        Assert.Equal("4", Assert.Single((await db.ExecuteAsync("SELECT parent FROM tree WHERE id=2"))[0].Rows)[0].Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM tree WHERE id=4"));
        Assert.Equal("3", (await db.ExecuteAsync("SELECT COUNT(*) FROM tree"))[0].Rows[0][0].Display);
        await db.ExecuteAsync("DELETE FROM guard; DELETE FROM tree WHERE id=4");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM tree"))[0].Rows);
    }

    [Fact]
    public async Task Swapping_parent_keys_updates_each_child_once_and_preserves_primary_lookups()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE p(id INT PRIMARY KEY, tag TEXT);
            CREATE TABLE c(tag TEXT, fk INT REFERENCES p(id) ON UPDATE CASCADE);
            INSERT INTO p VALUES(1,'a'),(2,'b');
            INSERT INTO c VALUES('a',1),('b',2);
            UPDATE p SET id=3-id
            """);
        Assert.Equal("b", Assert.Single((await db.ExecuteAsync("SELECT tag FROM p WHERE id=1"))[0].Rows)[0].Display);
        Assert.Equal("a", Assert.Single((await db.ExecuteAsync("SELECT tag FROM p WHERE id=2"))[0].Rows)[0].Display);
        Assert.Equal("a", Assert.Single((await db.ExecuteAsync("SELECT tag FROM c WHERE fk=2"))[0].Rows)[0].Display);
        Assert.Equal("b", Assert.Single((await db.ExecuteAsync("SELECT tag FROM c WHERE fk=1"))[0].Rows)[0].Display);
    }

    [Fact]
    public async Task Alter_and_drop_columns_validate_existing_values_and_catalog_dependencies()
    {
        var db = Database();
        await db.ExecuteAsync("""
            CREATE TABLE stock(id INT PRIMARY KEY, code VARCHAR(5), ignored INT);
            CREATE INDEX discard ON stock(ignored);
            INSERT INTO stock VALUES(1,NULL,7);
            ALTER TABLE stock ALTER COLUMN code SET DEFAULT 'abc';
            INSERT INTO stock(id) VALUES(2);
            """);
        foreach (var sql in new[] { "ALTER COLUMN code SET NOT NULL", "ALTER COLUMN code SET DATA TYPE VARCHAR(2)",
                                    "ALTER COLUMN id DROP NOT NULL" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE stock " + sql));
        await db.ExecuteAsync("""
            UPDATE stock SET code='ok' WHERE id=1;
            ALTER TABLE stock ALTER COLUMN code SET NOT NULL;
            ALTER TABLE stock ALTER COLUMN code DROP DEFAULT;
            ALTER TABLE stock DROP COLUMN ignored;
            ALTER TABLE stock ALTER COLUMN code SET DATA TYPE VARCHAR(20);
            CREATE VIEW codes AS SELECT code FROM stock
            """);
        Assert.Equal(new[] { "id", "code" }, (await db.ExecuteAsync("SELECT * FROM stock"))[0].Columns);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DROP INDEX discard ON stock"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO stock(id) VALUES(3)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE stock DROP COLUMN code"));
        await db.ExecuteAsync("DROP VIEW codes; ALTER TABLE stock ALTER COLUMN code DROP NOT NULL; INSERT INTO stock(id) VALUES(3)");
    }

    [Fact]
    public async Task Check_accepts_unknown_and_rejects_entire_statement_on_false()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE items(id INT PRIMARY KEY, qty INT CHECK (qty >= 0), limit_qty INT, CHECK(qty <= limit_qty)); INSERT INTO items VALUES(1,NULL,5),(2,2,3)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO items VALUES(3,3,4),(4,5,4)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE items SET qty=-1"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE items RENAME COLUMN qty TO amount"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE items ADD COLUMN z INT DEFAULT 0 CHECK (z>0)"));
        Assert.Equal("2", (await db.ExecuteAsync("SELECT COUNT(*) FROM items"))[0].Rows[0][0].Display);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE invalid(id INT, CHECK(missing>0))"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE invalid(id INT CHECK(id))"));
    }

    [Fact]
    public async Task Foreign_keys_check_new_child_and_restrict_parent_changes_within_transaction()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE p(id INT PRIMARY KEY, code TEXT UNIQUE); CREATE TABLE c(id INT, code TEXT, FOREIGN KEY(code) REFERENCES p(code)); BEGIN; INSERT INTO p VALUES(1,'A'); INSERT INTO c VALUES(1,'A'),(2,NULL)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM p WHERE id=1"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE p SET code='B'"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("UPDATE c SET code='B'"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DROP TABLE p"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE p RENAME COLUMN code TO other"));
        await db.ExecuteAsync("ROLLBACK");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO c VALUES(3,'A')"));
        await db.ExecuteAsync("DROP TABLE c; DROP TABLE p");
    }

    [Fact]
    public async Task Self_references_use_complete_statement_state_and_allow_deleting_entire_tree()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE tree(id INT PRIMARY KEY, parent INT REFERENCES tree(id)); INSERT INTO tree VALUES(2,1),(1,NULL)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM tree WHERE id=1"));
        await db.ExecuteAsync("DELETE FROM tree; INSERT INTO tree VALUES(3,3); DROP TABLE tree");
    }

    [Fact]
    public async Task Added_foreign_key_validates_parent_definition_and_existing_rows()
    {
        var db = Database();
        await db.ExecuteAsync("CREATE TABLE p(id INT PRIMARY KEY, other INT); CREATE TABLE c(id INT); INSERT INTO c VALUES(1)");
        foreach (var clause in new[] { "x INT REFERENCES missing(id)", "x INT REFERENCES p(other)", "x TEXT REFERENCES p(id)", "x INT DEFAULT 4 REFERENCES p(id)" })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE c ADD COLUMN " + clause));
        await db.ExecuteAsync("INSERT INTO p VALUES(4,7); ALTER TABLE c ADD COLUMN x INT DEFAULT 4 REFERENCES p(id)");
        Assert.Equal("4", (await db.ExecuteAsync("SELECT x FROM c"))[0].Rows[0][0].Display);
    }
}
