using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ReadOnlyPlanTests
{
    private static SqlTable Schema(string name) =>
        new(name, [new("id", "INTEGER", false, false, false, null, null)]);

    private static ExplainQuery Parse(string sql) => Assert.IsType<ExplainQuery>(Assert.Single(SqlParser.Parse(sql)));

    [Theory]
    [InlineData("SELECT 1/0 FROM books", "ARCHIVE SCAN")]
    [InlineData("SELECT a.id FROM books a JOIN copies c ON a.id=c.id", "JOIN INPUT")]
    [InlineData("WITH chosen AS (SELECT 1/0 AS value FROM books) SELECT value FROM chosen", "MATERIALIZE CTE")]
    public async Task Explain_uses_schema_and_manifest_counts_without_opening_any_reader(string sql, string expectedStep)
    {
        var result = await SqlDatabase.ExplainReadOnlyAsync(Parse("EXPLAIN " + sql), Schema,
            _ => throw new InvalidOperationException("EXPLAIN opened user rows"), _ => 10_000);
        Assert.Contains(result.Rows, row => row[1].Text == expectedStep);
        Assert.Contains(result.Rows, row => row[1].Text == "ARCHIVE SCAN" && row[4].Text == "10000");
    }

    [Fact]
    public async Task Manifest_count_reads_no_user_rows_and_returns_the_full_64_bit_count()
    {
        const long headerCount = 5_000_000_000;
        IAsyncEnumerable<SqlRow> NoScan(string _) => throw new InvalidOperationException("COUNT opened user rows");
        var plan = await SqlDatabase.ExplainReadOnlyAsync(Parse("EXPLAIN ANALYZE SELECT COUNT(*) FROM books"), Schema, NoScan, _ => headerCount);
        Assert.Contains(plan.Rows, row => row[1].Text == "ARCHIVE ROW COUNT" && row[6].Text == "1" && row[7].Text == "1");
        Assert.DoesNotContain(plan.Rows, row => row[1].Text == "ARCHIVE SCAN");
        var result = await SqlDatabase.QueryReadOnlyAsync(Parse("EXPLAIN SELECT COUNT(*) FROM books").Query, Schema, NoScan, _ => headerCount);
        Assert.Equal("5000000000", Assert.Single(Assert.Single(result.Rows)).Text);
    }

    [Fact]
    public async Task Analyzed_row_estimates_do_not_replace_a_live_count_when_the_manifest_count_is_unknown()
    {
        var table = Schema("books") with { Statistics = new(90_000, new()) };
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            await Task.Yield();
            for (int n = 0; n < 4100; n++)
                yield return new(n.ToString(), new() { ["id"] = SqlValue.Number(n.ToString()) });
        }
        var statement = Parse("EXPLAIN ANALYZE SELECT COUNT(*) FROM books");
        var plan = await SqlDatabase.ExplainReadOnlyAsync(statement, _ => table, Scan);
        var scan = Assert.Single(plan.Rows, row => row[1].Text == "ARCHIVE SCAN");
        Assert.Equal(("90000", "4100", "1"), (scan[4].Text, scan[6].Text, scan[7].Text));
        var result = await SqlDatabase.QueryReadOnlyAsync(statement.Query, _ => table, Scan);
        Assert.Equal("4100", Assert.Single(Assert.Single(result.Rows)).Text);
    }

    [Fact]
    public async Task Timed_streaming_filter_reads_only_the_prefix_needed_for_pagination()
    {
        int starts = 0, visits = 0, disposed = 0;
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            starts++;
            try
            {
                await Task.Yield();
                for (int n = 1; n <= 100; n++)
                {
                    visits++;
                    yield return new(n.ToString(), new() { ["id"] = SqlValue.Number(n.ToString()) });
                }
            }
            finally { disposed++; }
        }
        var plan = await SqlDatabase.ExplainReadOnlyAsync(
            Parse("EXPLAIN ANALYZE SELECT id FROM books WHERE id>3 LIMIT 2 OFFSET 1"), Schema, Scan);
        Assert.Equal((1, 6, 1), (starts, visits, disposed));
        Assert.Contains(plan.Rows, row => row[1].Text == "ARCHIVE SCAN" && row[6].Text == "6" && row[7].Text == "1");
        var filter = Assert.Single(plan.Rows, row => row[1].Text == "FILTER");
        Assert.Equal(("3", "1"), (filter[6].Text, filter[7].Text));
        Assert.Contains(plan.Rows, row => row[1].Text == "LIMIT" && row[6].Text == "2");
        var zero = await SqlDatabase.ExplainReadOnlyAsync(Parse("EXPLAIN ANALYZE SELECT id FROM books LIMIT 0"), Schema, Scan);
        Assert.Equal((1, 6, 1), (starts, visits, disposed));
        Assert.Contains(zero.Rows, row => row[1].Text == "ARCHIVE SCAN" && row[6].Text == "0" && row[7].Text == "0");
    }
}
