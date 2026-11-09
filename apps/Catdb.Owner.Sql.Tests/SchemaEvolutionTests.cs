using Catdb.Sql;
using System.Text.Json.Nodes;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class SchemaEvolutionTests
{
    [Fact]
    public async Task Legacy_foreign_keys_and_composite_cascades_survive_reopen_and_child_column_rename()
    {
        var directory = Directory.CreateTempSubdirectory("catdb-fk-catalog-");
        var path = Path.Combine(directory.FullName, "catalog.ndb");
        try
        {
            using (var store = await EmbeddedCatdbStore.OpenAsync(path, create: true))
            {
                var catalogPrefix = SqlNames.Hex("_catdb_owner/sql/v1/table/");
                var viewPrefix = SqlNames.Hex("_catdb_owner/sql/v1/view/");
                var oldFormat = new SqlDatabase(request =>
                {
                    var key = (string?)request["key"] ?? "";
                    if ((string?)request["op"] == "put" && key.StartsWith(catalogPrefix, StringComparison.Ordinal))
                    {
                        var schema = JsonNode.Parse(Convert.FromHexString((string)request["value"]!))!.AsObject();
                        schema.Remove("ForeignKeys");
                        foreach (var column in schema["Columns"]!.AsArray())
                            if (column!["Reference"] is JsonObject reference)
                            {
                                reference.Remove("UpdateCascade");
                                reference.Remove("DeleteCascade");
                            }
                        request["value"] = SqlNames.Hex(schema.ToJsonString());
                    }
                    if ((string?)request["op"] == "put" && key.StartsWith(viewPrefix, StringComparison.Ordinal))
                    {
                        var schema = JsonNode.Parse(Convert.FromHexString((string)request["value"]!))!.AsObject();
                        schema["Columns"]![0]!["Type"] = "REAL";
                        request["value"] = SqlNames.Hex(schema.ToJsonString());
                    }
                    return store.RequestAsync(request);
                }, () => store.InTransaction);
                await oldFormat.ExecuteAsync("""
                    CREATE TABLE p(id INT PRIMARY KEY);
                    CREATE TABLE legacy(fk INT REFERENCES p(id));
                    CREATE VIEW old_constant AS SELECT 1 AS n FROM legacy;
                    INSERT INTO p VALUES(1);
                    INSERT INTO legacy VALUES(1);
                    """);
            }
            using (var db = await CatdbSqlConnection.OpenAsync(path))
            {
                await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("DELETE FROM p"));
                await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("ALTER TABLE p RENAME COLUMN id TO new_id"));
                await db.ExecuteAsync("ALTER TABLE legacy ADD COLUMN extra INT");
                await db.ExecuteAsync("""
                    CREATE TABLE pairs(a INT, b INT, PRIMARY KEY(a,b));
                    CREATE TABLE links(x INT, y INT, extra INT, FOREIGN KEY(x,y) REFERENCES pairs(a,b) ON UPDATE CASCADE ON DELETE CASCADE);
                    CREATE INDEX link_key ON links(x,y);
                    INSERT INTO pairs VALUES(1,2);
                    INSERT INTO links VALUES(1,2,0);
                    ALTER TABLE links RENAME COLUMN x TO renamed;
                    ALTER TABLE links DROP COLUMN extra
                    """);
            }
            using var reopened = await CatdbSqlConnection.OpenAsync(path);
            await reopened.ExecuteAsync("UPDATE pairs SET b=3");
            Assert.Single((await reopened.ExecuteAsync("SELECT * FROM links WHERE renamed=1 AND y=3"))[0].Rows);
            await Assert.ThrowsAsync<SqlException>(() => reopened.ExecuteAsync("ALTER TABLE links DROP COLUMN renamed"));
            await reopened.ExecuteAsync("DELETE FROM pairs");
            Assert.Empty((await reopened.ExecuteAsync("SELECT * FROM links"))[0].Rows);
        }
        finally { directory.Delete(recursive: true); }
    }

    [Fact]
    public async Task Composite_key_and_column_changes_preserve_row_identity_after_reopen()
    {
        var directory = Directory.CreateTempSubdirectory("catdb-schema-");
        var path = Path.Combine(directory.FullName, "schema.ndb");
        try
        {
            using (var db = await CatdbSqlConnection.OpenAsync(path, true))
            {
                await db.ExecuteAsync("CREATE TABLE inventory(site TEXT, sku REAL, PRIMARY KEY(site,sku)); INSERT INTO inventory VALUES ('A',1),('A',2),('B',1);");
                foreach (var invalid in new[] { "INSERT INTO inventory VALUES ('A',1.00)", "INSERT INTO inventory VALUES (NULL,9)", "UPDATE inventory SET site='A' WHERE site='B'", "DROP INDEX [PRIMARY] ON inventory" })
                    await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(invalid));
                await db.ExecuteAsync("ALTER TABLE inventory ADD COLUMN quantity INT NOT NULL DEFAULT 5; ALTER TABLE inventory RENAME COLUMN site TO warehouse; REINDEX inventory");
                await db.ExecuteAsync("BEGIN; ALTER TABLE inventory RENAME COLUMN sku TO lost; ROLLBACK");
            }
            using var reopened = await CatdbSqlConnection.OpenAsync(path);
            var result = (await reopened.ExecuteAsync("SELECT quantity FROM inventory WHERE warehouse='A' AND sku=1.0"))[0];
            Assert.Equal("5", Assert.Single(Assert.Single(result.Rows)).Display);
            await Assert.ThrowsAsync<SqlException>(() => reopened.ExecuteAsync("INSERT INTO inventory(warehouse,sku) VALUES ('A',2.00)"));
        }
        finally { directory.Delete(recursive: true); }
    }
}
