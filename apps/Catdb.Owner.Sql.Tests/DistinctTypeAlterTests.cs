using System.Text.Json.Nodes;
using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class DistinctTypeAlterTests
{
    private const string UpperId = "AA000000-1111-2222-3333-444444444444";
    private const string LowerId = "aa000000-1111-2222-3333-444444444444";
    private const string ChangeType = "ALTER TABLE accounts ALTER COLUMN id TYPE AccountId";

    [Fact]
    public async Task Changing_an_indexed_text_key_to_a_named_uuid_replaces_both_indexes_and_survives_reopen()
    {
        var directory = Directory.CreateTempSubdirectory("catdb-type-change-");
        var path = Path.Combine(directory.FullName, "types.ndb");
        try
        {
            using (var session = await CatdbSqlConnection.OpenAsync(path, create: true))
            {
                await session.ExecuteAsync($"""
                    CREATE TYPE AccountId AS DISTINCT UUID;
                    CREATE TABLE accounts(id TEXT PRIMARY KEY);
                    CREATE INDEX by_id ON accounts(id);
                    INSERT INTO accounts VALUES ('{UpperId}');
                    BEGIN; {ChangeType}; ROLLBACK
                    """);
                var original = Assert.Single(await session.ExecuteAsync($"SELECT id FROM accounts WHERE id='{UpperId}'"));
                Assert.Equal("TEXT", Assert.Single(original.ColumnTypes!));
                Assert.Equal(UpperId, Assert.Single(Assert.Single(original.Rows)).Text);
                await session.ExecuteAsync(ChangeType);
            }
            using var store = await EmbeddedCatdbStore.OpenAsync(path);
            var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
            foreach (var comparison in new[] { "=", ">=" })
            {
                var result = Assert.Single(await db.ExecuteAsync($"SELECT id FROM accounts WHERE id {comparison} CAST('{LowerId}' AS AccountId)"));
                Assert.Equal("AccountId", Assert.Single(result.ColumnTypes!));
                var value = Assert.Single(Assert.Single(result.Rows));
                Assert.Equal("AccountId", value.TypeName);
                Assert.Equal(LowerId, value.Text);
            }
            await db.ExecuteAsync("BEGIN");
            foreach (var index in new[] { "pk", "index" })
            {
                var prefix = "_catdb_owner/sql/v1/" + index + "/";
                var page = await store.RequestAsync(new JsonObject { ["op"] = "scan", ["start"] = SqlNames.Hex(prefix), ["end"] = SqlNames.Hex(prefix + "~") });
                Assert.Single(page["rows"]!.AsArray());
            }
            await db.ExecuteAsync("ROLLBACK");
        }
        finally { directory.Delete(recursive: true); }
    }

    [Fact]
    public async Task An_alter_that_would_merge_two_primary_keys_preserves_the_original_rows_and_type()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync($"""
            CREATE TYPE AccountId AS DISTINCT UUID;
            CREATE TABLE accounts(id TEXT PRIMARY KEY);
            INSERT INTO accounts VALUES ('{UpperId}'), ('{LowerId}')
            """);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(ChangeType));
        foreach (var id in new[] { UpperId, LowerId })
        {
            var result = Assert.Single(await db.ExecuteAsync($"SELECT id FROM accounts WHERE id='{id}'"));
            Assert.Equal("TEXT", Assert.Single(result.ColumnTypes!));
            Assert.Equal(id, Assert.Single(Assert.Single(result.Rows)).Text);
        }
    }
}
