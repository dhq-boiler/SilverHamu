using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class LoadedQueryResultTests
{
    private static readonly SqlTable Table = new("Book_3", [new("Id", "INTEGER", false, false, false, null, null)]);

    [Theory]
    [InlineData(0)]
    [InlineData(99)]
    [InlineData(100)]
    [InlineData(101)]
    [InlineData(4095)]
    [InlineData(4096)]
    [InlineData(4097)]
    [InlineData(4201)]
    public async Task All_rows_are_loaded_before_paging_without_repeated_source_reads(int count)
    {
        int reads = 0, scans = 0;
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            scans++;
            await Task.CompletedTask;
            for (int i = 0; i < count; i++)
            {
                reads++;
                yield return new("", new() { ["Id"] = SqlValue.Number(i.ToString()) });
            }
        }
        using var loaded = await Load("SELECT * FROM Book_3", Scan);
        Assert.Equal(count, reads);
        Assert.Equal(count, loaded.RowCount);
        Assert.Equal(Math.Max(1, (count + 99) / 100), loaded.PageCount);
        var rows = new List<string>();
        for (long page = 0; page < loaded.PageCount; page++)
        {
            var result = loaded.ReadPage(page);
            Assert.Equal(new[] { "Id" }, result.Columns);
            Assert.True(result.Rows.Count <= 100);
            rows.AddRange(result.Rows.Select(row => row[0].Display));
        }
        Assert.Equal(Enumerable.Range(0, count).Select(i => i.ToString()), rows);
        Assert.Equal(Enumerable.Range(0, Math.Min(count, 100)).Select(i => i.ToString()),
            loaded.ReadPage(0).Rows.Select(r => r[0].Display));
        Assert.Equal(count, reads);
        Assert.Equal(1, scans);
        Assert.Throws<ArgumentOutOfRangeException>(() => loaded.ReadPage(-1));
        Assert.Throws<ArgumentOutOfRangeException>(() => loaded.ReadPage(loaded.PageCount));
    }

    [Theory]
    [InlineData("SELECT Id FROM Book_3 WHERE Id >= 4090", 110, "4090", "4199")]
    [InlineData("SELECT Id FROM Book_3 ORDER BY Id DESC", 4200, "4199", "0")]
    [InlineData("SELECT Id FROM Book_3 LIMIT 4097 OFFSET 1", 4097, "1", "4097")]
    [InlineData("SELECT Id FROM Book_3 FETCH FIRST 4097 ROWS ONLY", 4097, "0", "4096")]
    [InlineData("SELECT Id FROM Book_3 OFFSET 4096 ROWS FETCH NEXT 100 ROWS ONLY", 100, "4096", "4195")]
    [InlineData("SELECT Id FROM Book_3 ORDER BY Id DESC OFFSET 1 ROW FETCH FIRST 101 ROWS ONLY", 101, "4198", "4098")]
    [InlineData("SELECT Id FROM Book_3 FETCH FIRST ROW ONLY", 1, "0", "0")]
    [InlineData("SELECT Id FROM Book_3 FETCH NEXT 0 ROWS ONLY", 0, null, null)]
    [InlineData("SELECT Id FROM Book_3 LIMIT 0", 0, null, null)]
    [InlineData("SELECT COUNT(*) FROM Book_3", 1, "4200", "4200")]
    public async Task Loaded_results_preserve_filter_order_and_explicit_bounds(string sql, int count, string? first, string? last)
    {
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            await Task.CompletedTask;
            for (int i = 0; i < 4200; i++) yield return new("", new() { ["Id"] = SqlValue.Number(i.ToString()) });
        }
        using var loaded = await Load(sql, Scan);
        Assert.Equal(count, loaded.RowCount);
        Assert.Equal(first, loaded.ReadPage(0).Rows.FirstOrDefault()?[0].Display);
        Assert.Equal(last, loaded.ReadPage(loaded.PageCount - 1).Rows.LastOrDefault()?[0].Display);
    }

    [Fact]
    public async Task Large_offsets_do_not_wrap_or_read_from_the_start()
    {
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            await Task.CompletedTask;
            yield return new("", new() { ["Id"] = SqlValue.Number("1") });
        }
        using var loaded = await Load("SELECT * FROM Book_3 OFFSET 2147483647 ROWS", Scan);
        Assert.Equal(0, loaded.RowCount);
    }

    [Fact]
    public async Task Explain_analyze_measures_the_same_full_scan_as_an_unlimited_archive_select()
    {
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            await Task.CompletedTask;
            for (int i = 0; i < 4201; i++) yield return new("", new() { ["Id"] = SqlValue.Number(i.ToString()) });
        }
        var query = Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse("SELECT * FROM Book_3")));
        var result = await SqlDatabase.QueryReadOnlyAsync(query, _ => Table, Scan);
        Assert.Equal(4201, result.Rows.Count);
        var plan = await SqlDatabase.ExplainReadOnlyAsync(new(query, Analyze: true), _ => Table, Scan);
        int operation = plan.Columns.ToList().IndexOf("operation");
        int actual = plan.Columns.ToList().IndexOf("actual_rows");
        Assert.Equal("4201", Assert.Single(plan.Rows, row => row[operation].Display == "RESULT")[actual].Display);
        Assert.Equal("4201", Assert.Single(plan.Rows, row => row[operation].Display == "ARCHIVE SCAN")[actual].Display);
    }

    [Fact]
    public async Task Stored_values_preserve_null_empty_unicode_and_numeric_types_after_disposal_of_source()
    {
        var expected = new[] { SqlValue.Null, SqlValue.String(""), SqlValue.String("銀ハム🐹\n\0"), SqlValue.Number("12.5"), SqlValue.Boolean(true) };
        async IAsyncEnumerable<SqlResult> Batches()
        {
            await Task.CompletedTask;
            yield return new(["a", "b", "c", "d", "e"], [expected], "") { ColumnTypes = ["NULL", "TEXT", "TEXT", "REAL", "BOOLEAN"] };
        }
        var loaded = await LoadedQueryResult.LoadAsync(Batches());
        Assert.Equal(expected, Assert.Single(loaded.ReadPage(0).Rows));
        Assert.Equal(new[] { "NULL", "TEXT", "TEXT", "REAL", "BOOLEAN" }, loaded.ReadPage(0).ColumnTypes);
        loaded.Dispose();
        loaded.Dispose();
        Assert.Throws<ObjectDisposedException>(() => loaded.ReadPage(0));
    }

    [Fact]
    public async Task A_failure_after_a_full_batch_does_not_return_a_partial_success()
    {
        async IAsyncEnumerable<SqlRow> Scan(string _)
        {
            await Task.CompletedTask;
            for (int i = 0; i < 100; i++) yield return new("", new() { ["Id"] = SqlValue.Number(i.ToString()) });
            throw new IOException("source failed");
        }
        var error = await Assert.ThrowsAsync<IOException>(() => Load("SELECT * FROM Book_3", Scan));
        Assert.Equal("source failed", error.Message);
    }

    private static Task<LoadedQueryResult> Load(string sql, Func<string, IAsyncEnumerable<SqlRow>> scan) =>
        LoadedQueryResult.LoadAsync(SqlDatabase.ReadAllReadOnlyAsync(
            Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse(sql))), _ => Table, scan));
}
