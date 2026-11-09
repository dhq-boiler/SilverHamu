using System.Text.Json;
using System.Text.Json.Nodes;
using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class OrderedIndexTests
{
    [Fact]
    public async Task Composite_blob_range_survives_native_reopen()
    {
        var path = Path.Combine(Path.GetTempPath(), $"catdb-index-{Guid.NewGuid():N}.ndb");
        try
        {
            using (var db = await CatdbSqlConnection.OpenAsync(path, create: true))
                await db.ExecuteAsync("CREATE TABLE t(id INT,flag BOOLEAN,key BLOB); CREATE INDEX combined ON t(flag,key); INSERT INTO t VALUES (1,TRUE,X''),(2,TRUE,X'00'),(3,TRUE,X'00FF'),(4,FALSE,X'00FF')");
            using var reopened = await CatdbSqlConnection.OpenAsync(path);
            var result = (await reopened.ExecuteAsync("SELECT id FROM t WHERE flag=TRUE AND key>=X'00' AND key<X'01' ORDER BY id"))[0];
            Assert.Equal(new[] { "2","3" }, result.Rows.Select(r => r[0].Display));
        }
        finally { File.Delete(path); File.Delete(path + ".owner"); }
    }

    [Fact]
    public async Task Composite_prefix_range_preserves_null_suffix_and_exact_decimal_order()
    {
        var store = new MemorySqlStore();
        var rowScans = 0;
        var rowPrefix = SqlNames.Hex("_catdb_owner/sql/v1/row/");
        var db = new SqlDatabase(request =>
        {
            if (request["op"]!.ToString() == "scan" && request["start"]!.ToString().StartsWith(rowPrefix))
                rowScans++;
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("""
            CREATE TABLE t(id INT, region TEXT, price REAL);
            INSERT INTO t VALUES (1,'A',-79228162514264337593543950335),(2,'A',-0.0000000000000000000000000001),
              (3,'A',0),(4,'A',0.0100),(5,'A',79228162514264337593543950335),(6,'A',NULL),(7,'B',0.005);
            CREATE INDEX price_list ON t(region,price)
            """);
        async Task<int[]> Ids(string condition) =>
            (await db.ExecuteAsync($"SELECT id FROM t WHERE {condition} ORDER BY id"))[0].Rows.Select(r => int.Parse(r[0].Display)).ToArray();
        rowScans = 0;
        Assert.Equal(new[] { 1, 2, 3, 4, 5, 6 }, await Ids("region='A'"));
        Assert.Equal(new[] { 2,3 }, await Ids("region='A' AND -1<price AND price<0.01"));
        Assert.Equal(new[] { 3,4,5 }, await Ids("price>=0 AND region='A'"));
        Assert.Equal(new[] { 4 }, await Ids("price=0.01 AND region='A'"));
        Assert.Empty(await Ids("region='A' AND price<0 AND price>1"));
        Assert.Equal(0, rowScans);
        await db.ExecuteAsync("BEGIN; UPDATE t SET region='B',price=3 WHERE id=2; DELETE FROM t WHERE id=3");
        Assert.Equal(new[] { 2 }, await Ids("region='B' AND price>=3"));
        Assert.Empty(await Ids("region='A' AND price>-1 AND price<0.01"));
        await db.ExecuteAsync("ROLLBACK; ALTER TABLE t RENAME COLUMN price TO amount; REINDEX t");
        Assert.Equal(new[] { 2,3 }, await Ids("region='A' AND amount>-1 AND amount<0.01"));
    }

    [Fact]
    public async Task Ordinal_text_range_rechecks_truncated_boundary_keys()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        var prefix = new string('猫', 400);
        await db.ExecuteAsync($"CREATE TABLE t(id INT,k TEXT); CREATE INDEX names ON t(k); INSERT INTO t VALUES (1,'{prefix}a'),(2,'{prefix}b'),(3,'{prefix}c'),(4,'𐀀'),(5,'')");
        foreach (var condition in new[] { $"k>'{prefix}b'", $"k='{prefix}b'", "k>='𐀀' AND k<''" })
        {
            var indexed = (await db.ExecuteAsync($"SELECT id FROM t WHERE {condition} ORDER BY id"))[0].Rows;
            var scanned = (await db.ExecuteAsync($"SELECT id FROM t WHERE ({condition}) OR FALSE ORDER BY id"))[0].Rows;
            Assert.Equal(scanned.Select(r => r[0].Display), indexed.Select(r => r[0].Display));
        }
    }

    [Fact]
    public async Task Legacy_hash_index_remains_writable_and_reindex_atomically_enables_range_search()
    {
        var store = new MemorySqlStore();
        bool rowScan = false, legacySchema = true;
        var db = new SqlDatabase(request =>
        {
            var op = request["op"]!.ToString();
            if (op == "scan" && request["start"]!.ToString().StartsWith(SqlNames.Hex("_catdb_owner/sql/v1/row/"))) rowScan = true;
            if (op == "put" && legacySchema && request["key"]!.ToString().StartsWith(SqlNames.Hex("_catdb_owner/sql/v1/table/")))
            {
                var schema = JsonNode.Parse(Convert.FromHexString(request["value"]!.ToString()))!.AsObject();
                if (schema["Indexes"] is JsonArray indexes)
                {
                    foreach (var index in indexes) index!.AsObject().Remove("Columns");
                    request["value"] = Convert.ToHexString(JsonSerializer.SerializeToUtf8Bytes(schema));
                }
            }
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t(v INT); CREATE INDEX idx ON t(v); INSERT INTO t VALUES (1),(2)");
        rowScan = false;
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t WHERE v=1"))[0].Rows);
        Assert.False(rowScan);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t WHERE v>1"))[0].Rows);
        Assert.True(rowScan);
        legacySchema = false;
        await db.ExecuteAsync("BEGIN; REINDEX t; ROLLBACK; REINDEX t");
        rowScan = false;
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t WHERE v>1"))[0].Rows);
        Assert.False(rowScan);
    }
}
