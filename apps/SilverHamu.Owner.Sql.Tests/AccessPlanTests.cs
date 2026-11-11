using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class AccessPlanTests
{
    [Fact]
    public async Task Value_frequency_changes_the_plan_and_the_scan_actually_sent_to_storage()
    {
        var store = new MemorySqlStore();
        var scans = new List<string>();
        var db = new SqlDatabase(request =>
        {
            if (request["op"]!.ToString() == "scan") scans.Add(request["start"]!.ToString());
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE events (id INT PRIMARY KEY, bucket INT); CREATE INDEX by_bucket ON events(bucket)");
        var tuples = Enumerable.Range(1, 64).Select(i => $"({i},{(i == 64 ? 1 : 0)})");
        await db.ExecuteAsync("INSERT INTO events VALUES " + string.Join(',', tuples));

        async Task Verify(int bucket, bool useIndex)
        {
            var sql = $"SELECT id FROM events WHERE bucket={bucket}";
            var explain = Assert.Single(await db.ExecuteAsync("EXPLAIN " + sql));
            string operation = useIndex ? "INDEX RANGE SCAN" : "TABLE SCAN";
            Assert.Contains(explain.Rows, row => row.Any(cell => cell.Text == operation));
            scans.Clear();
            var result = Assert.Single(await db.ExecuteAsync(sql));
            Assert.Equal(bucket == 1 ? 1 : 63, result.Rows.Count);
            Assert.Contains(scans, key => key.StartsWith(SqlNames.Hex(useIndex ? "_catdb_owner/sql/v1/index/" : "_catdb_owner/sql/v1/row/"), StringComparison.Ordinal));
        }

        await Verify(0, useIndex: true);
        await db.ExecuteAsync("BEGIN; ANALYZE events");
        await Verify(0, useIndex: false);
        await Verify(1, useIndex: true);
        await db.ExecuteAsync("ROLLBACK");
        await Verify(0, useIndex: true);
    }

    [Fact]
    public async Task Primary_lookup_with_a_second_condition_still_filters_the_fetched_row()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE t (id INT PRIMARY KEY, score INT); INSERT INTO t VALUES (1,2),(2,9)");
        var missing = Assert.Single(await db.ExecuteAsync("SELECT id FROM t WHERE id=1 AND score>2"));
        Assert.Empty(missing.Rows);
        var hit = Assert.Single(await db.ExecuteAsync("SELECT id FROM t WHERE score>2 AND id=2"));
        Assert.Equal("2", Assert.Single(Assert.Single(hit.Rows)).Text);
    }
}
