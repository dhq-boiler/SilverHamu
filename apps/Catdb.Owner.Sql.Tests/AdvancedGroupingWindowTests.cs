using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class AdvancedGroupingWindowTests
{
    private static async Task<SqlDatabase> Events()
    {
        var store = new MemorySqlStore();
        var db = new SqlDatabase(store.RequestAsync, () => store.InTransaction);
        await db.ExecuteAsync("""
            CREATE TABLE e(id INTEGER PRIMARY KEY,k REAL,v INTEGER);
            INSERT INTO e VALUES(1,NULL,0),(2,1,3),(3,1,4),(4,1.5,6),(5,4,8),(6,NULL,0)
            """);
        return db;
    }

    private static string[] Rows(SqlResult result) =>
        result.Rows.Select(row => string.Join(":", row.Select(v => v.Display))).ToArray();

    [Fact]
    public async Task Grouping_uses_expression_values_and_filters_each_aggregate_before_its_argument()
    {
        var db = await Events();
        var result = await db.ExecuteAsync("""
            SELECT CASE WHEN v IN (0,3) THEN 0 ELSE 1 END AS bucket,
                   COUNT(*) FILTER (WHERE k IS NOT NULL) AS c,
                   SUM(12/v) FILTER (WHERE v<>0) AS s
            FROM e GROUP BY CASE WHEN v IN (0,3) THEN 0 ELSE 1 END
            HAVING COUNT(*) FILTER (WHERE v>0)>0 ORDER BY bucket
            """);
        Assert.Equal(new[] { "0:1:4", "1:3:6.5" }, Rows(result.Single()));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT v FROM e GROUP BY v%2"));
        var nested = await db.ExecuteAsync("SELECT (id%2)+1 AS bucket,COUNT(*) FROM e GROUP BY id%2 ORDER BY bucket");
        Assert.Equal(new[] { "1:3", "2:3" }, Rows(nested.Single()));
    }

    [Fact]
    public async Task Range_offsets_use_key_distances_while_groups_count_peer_groups()
    {
        var db = await Events();
        const string query = """
            SELECT id,
                COUNT(*) OVER (ORDER BY k NULLS LAST RANGE BETWEEN 0.5 PRECEDING AND CURRENT ROW) AS nearby,
                COUNT(*) OVER (ORDER BY k DESC NULLS FIRST GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW) AS neighbors,
                COUNT(*) OVER (ORDER BY k DESC NULLS FIRST RANGE BETWEEN CURRENT ROW AND 0.5 FOLLOWING) AS descending_nearby
            FROM e ORDER BY id
            """;
        var result = Assert.Single(await db.ExecuteAsync(query));
        Assert.Equal(new[] { "1:2:2:2","2:2:3:2","3:2:3:2","4:3:2:3","5:1:3:1","6:2:2:2" }, Rows(result));
        var plan = Assert.Single(await db.ExecuteAsync("EXPLAIN ANALYZE " + query));
        Assert.Equal(3, plan.Rows.Count(r => r[1].Text == "WINDOW"));
    }

    [Fact]
    public async Task Window_filters_keep_excluded_rows_in_the_frame_but_skip_their_values()
    {
        var db = await Events();
        var result = await db.ExecuteAsync("""
            SELECT id, COUNT(*) FILTER (WHERE k IS NOT NULL) OVER (ORDER BY id ROWS 1 PRECEDING),
                SUM(12/v) FILTER (WHERE v<>0) OVER (ORDER BY id ROWS CURRENT ROW)
            FROM e ORDER BY id
            """);
        Assert.Equal(new[] { "1:0:NULL","2:1:4","3:2:3","4:2:2","5:2:1.5","6:1:NULL" }, Rows(result.Single()));
        var grouped = await db.ExecuteAsync("""
            SELECT id%2 AS parity, SUM(SUM(v)) FILTER (WHERE SUM(v)>10) OVER () AS big
            FROM e GROUP BY id%2 ORDER BY parity
            """);
        Assert.Equal(new[] { "0:12", "1:12" }, Rows(grouped.Single()));
    }

    [Fact]
    public async Task Fractional_range_bound_does_not_round_up_to_the_next_large_key()
    {
        var db = await Events();
        var result = await db.ExecuteAsync("""
            SELECT COUNT(*) OVER (ORDER BY k RANGE BETWEEN CURRENT ROW AND 0.9 FOLLOWING)
            FROM (SELECT 10000000000000000000000000000 AS k UNION ALL SELECT 10000000000000000000000000001) d
            ORDER BY k
            """);
        Assert.Equal(new[] { "1", "1" }, Rows(result.Single()));
    }

    [Fact]
    public async Task Reversed_offsets_in_the_same_direction_produce_empty_frames()
    {
        var db = await Events();
        var result = await db.ExecuteAsync("""
            SELECT COUNT(*) OVER (ORDER BY id GROUPS BETWEEN 1 PRECEDING AND 2 PRECEDING),
                   SUM(v) OVER (ORDER BY id RANGE BETWEEN 2 FOLLOWING AND 1 FOLLOWING)
            FROM e ORDER BY id
            """);
        Assert.Equal(Enumerable.Repeat("0:NULL", 6), Rows(result.Single()));
    }

    [Theory]
    [InlineData("SELECT k AS x FROM e ORDER BY x DESC NULLS FIRST,id", "NULL,NULL,4,1.5,1,1")]
    [InlineData("SELECT k AS x FROM e GROUP BY k ORDER BY x ASC NULLS LAST", "1,1.5,4,NULL")]
    [InlineData("SELECT NULL AS x UNION SELECT 5 ORDER BY x NULLS LAST", "5,NULL")]
    public async Task Explicit_null_order_is_used_in_each_sort_path(string query, string expected)
    {
        var db = await Events();
        Assert.Equal(expected, string.Join(",", Rows(Assert.Single(await db.ExecuteAsync(query)))));
    }

    [Fact]
    public async Task A_non_equi_full_join_preserves_unmatched_rows_and_explains_nested_loops()
    {
        var db = await Events();
        await db.ExecuteAsync("CREATE TABLE b(lo INT,hi INT); INSERT INTO b VALUES (2,4),(20,30)");
        const string query = "SELECT e.id,b.lo FROM e FULL JOIN b ON e.id BETWEEN b.lo AND b.hi ORDER BY e.id NULLS LAST";
        Assert.Equal(new[] { "1:NULL","2:2","3:2","4:2","5:NULL","6:NULL","NULL:20" },
            Rows(Assert.Single(await db.ExecuteAsync(query))));
        var plan = Assert.Single(await db.ExecuteAsync("EXPLAIN " + query));
        Assert.Contains(plan.Rows, row => row[1].Text == "NESTED LOOP JOIN");
    }

    [Fact]
    public async Task Archive_grouped_filtered_count_reads_rows_even_with_a_known_total()
    {
        var schema = new SqlTable("archive", [new("n", "INTEGER", false, false, false, null, null)]);
        async IAsyncEnumerable<SqlRow> ReadRows(string name)
        {
            await Task.Yield();
            foreach (string n in new[] { "1", "2", "3", "4" })
                yield return new(n, new() { ["n"] = SqlValue.Number(n) });
        }
        const string sql = "SELECT n%2 AS parity, COUNT(*) FILTER (WHERE n>2) FROM archive GROUP BY n%2 ORDER BY parity";
        var query = Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse(sql)));
        var result = await SqlDatabase.QueryReadOnlyAsync(query, _ => schema, ReadRows, _ => 4);
        Assert.Equal(new[] { "0:1", "1:1" }, Rows(result));
        var explain = Assert.IsType<ExplainQuery>(Assert.Single(SqlParser.Parse("EXPLAIN ANALYZE " + sql)));
        var profile = await SqlDatabase.ExplainReadOnlyAsync(explain, _ => schema, ReadRows, _ => 4);
        Assert.Contains(profile.Rows, row => row[1].Text == "ARCHIVE SCAN");
    }

    [Theory]
    [InlineData("SELECT COUNT(*) FILTER (WHERE 1) FROM e")]
    [InlineData("SELECT COUNT(*) FILTER (WHERE SUM(v)>0) FROM e")]
    [InlineData("SELECT ROW_NUMBER() FILTER (WHERE TRUE) OVER () FROM e")]
    [InlineData("SELECT SUM(v) OVER (GROUPS 1 PRECEDING) FROM e")]
    [InlineData("SELECT SUM(v) OVER (ORDER BY id,k RANGE 1 PRECEDING) FROM e")]
    [InlineData("SELECT SUM(v) OVER (ORDER BY id GROUPS 0.5 PRECEDING) FROM e")]
    [InlineData("SELECT SUM(v) OVER (ORDER BY id RANGE -1 PRECEDING) FROM e")]
    [InlineData("SELECT v FROM e GROUP BY SUM(v)")]
    public async Task Invalid_window_and_group_definitions_fail_when_explained(string query)
    {
        var db = await Events();
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("EXPLAIN " + query));
    }
}
