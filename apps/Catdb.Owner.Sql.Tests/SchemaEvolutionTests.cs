using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class SchemaEvolutionTests
{
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
