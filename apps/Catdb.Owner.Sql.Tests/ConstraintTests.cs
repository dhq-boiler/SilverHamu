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
