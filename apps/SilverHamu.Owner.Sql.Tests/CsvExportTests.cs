using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class CsvExportTests : IDisposable
{
    private readonly DirectoryInfo directory = Directory.CreateTempSubdirectory("silverhamu-csv-");
    private string PathFor(string name = "result.csv") => Path.Combine(directory.FullName, name);

    [Theory]
    [InlineData(205, nameof(CsvExportScope.All), 0, 100, 1, 0, 0, 205)]
    [InlineData(205, nameof(CsvExportScope.CurrentPage), 0, 100, 1, 0, 0, 100)]
    [InlineData(205, nameof(CsvExportScope.CurrentPage), 1, 100, 1, 0, 100, 100)]
    [InlineData(205, nameof(CsvExportScope.CurrentPage), 2, 100, 1, 0, 200, 5)]
    [InlineData(205, nameof(CsvExportScope.CurrentPage), 0, 205, 1, 0, 0, 205)]
    [InlineData(205, nameof(CsvExportScope.Range), 0, 100, 99, 102, 98, 4)]
    [InlineData(205, nameof(CsvExportScope.Range), 0, 100, 205, 205, 204, 1)]
    [InlineData(205, nameof(CsvExportScope.Range), 0, 100, 1, 205, 0, 205)]
    [InlineData(0, nameof(CsvExportScope.All), 0, 100, 1, 0, 0, 0)]
    [InlineData(0, nameof(CsvExportScope.CurrentPage), 0, 100, 1, 0, 0, 0)]
    public void Scope_resolves_to_absolute_zero_based_rows_with_inclusive_user_bounds(long total, string scope,
        long page, int pageSize, long first, long last, long start, long count) =>
        Assert.Equal(new CsvExportRange(start, count), CsvExportRange.Create(total, Enum.Parse<CsvExportScope>(scope), page, pageSize, first, last));

    [Theory]
    [InlineData(0, 1)]
    [InlineData(-1, 1)]
    [InlineData(2, 1)]
    [InlineData(1, 206)]
    [InlineData(206, 206)]
    [InlineData(1, long.MaxValue)]
    public void Out_of_bounds_and_reversed_ranges_are_rejected(long first, long last) =>
        Assert.Throws<ArgumentException>(() => CsvExportRange.Create(205, CsvExportScope.Range, 0, 100, first, last));

    [Theory]
    [InlineData(-1)]
    [InlineData(3)]
    [InlineData(long.MaxValue)]
    public void Invalid_pages_are_rejected_before_offset_multiplication(long page) =>
        Assert.Throws<ArgumentException>(() => CsvExportRange.Create(205, CsvExportScope.CurrentPage, page, 100));

    [Fact]
    public async Task All_cached_rows_and_ranges_past_4096_export_without_rescanning_the_source()
    {
        int reads = 0;
        async IAsyncEnumerable<SqlResult> Batches()
        {
            await Task.CompletedTask;
            for (int start = 0; start < 4201; start += 100)
            {
                var rows = Enumerable.Range(start, Math.Min(100, 4201 - start)).Select(i =>
                {
                    reads++;
                    return new[] { SqlValue.Number(i.ToString()), SqlValue.String($"row{i}") };
                }).ToArray();
                yield return new(["id", "label"], rows, "");
            }
        }
        using var loaded = await LoadedQueryResult.LoadAsync(Batches());
        Assert.Equal(4201, reads);
        Assert.Equal(4201, await CsvExport.WriteAsync(PathFor(), ["id", "label"], loaded.ReadRows(0, loaded.RowCount)));
        Assert.Equal(4202, (await File.ReadAllLinesAsync(PathFor())).Length);
        var range = CsvExportRange.Create(4201, CsvExportScope.Range, 0, 100, 4097, 4201);
        Assert.Equal(105, await CsvExport.WriteAsync(PathFor("tail.csv"), ["id", "label"], loaded.ReadRows(range.Start, range.Count)));
        var tail = await File.ReadAllLinesAsync(PathFor("tail.csv"));
        Assert.Equal("\"4096\",\"row4096\"", tail[1]);
        Assert.Equal("\"4200\",\"row4200\"", tail[^1]);
        Assert.Equal(new[] { "99", "100", "101" }, loaded.ReadRows(99, 3).Select(row => row[0].Display));
        Assert.Equal(4201, reads);
        Assert.Equal("0", loaded.ReadPage(0).Rows[0][0].Display);
    }

    [Fact]
    public async Task Csv_preserves_utf8_bom_headers_quotes_commas_embedded_newlines_and_display_values()
    {
        var rows = new[] { new[] { SqlValue.String("銀ハム,\r\n\"テスト\""), SqlValue.Null, SqlValue.Number("1.23") } };
        Assert.Equal(1, await CsvExport.WriteAsync(PathFor(), ["名,前", "quote\"", "line\nheader"], rows));
        var bytes = await File.ReadAllBytesAsync(PathFor());
        Assert.Equal(new byte[] { 0xEF, 0xBB, 0xBF }, bytes.Take(3));
        Assert.Equal("\"名,前\",\"quote\"\"\",\"line\nheader\"\r\n\"銀ハム,\r\n\"\"テスト\"\"\",\"NULL\",\"1.23\"\r\n", await File.ReadAllTextAsync(PathFor()));
    }

    [Fact]
    public async Task Zero_rows_export_only_the_header()
    {
        Assert.Equal(0, await CsvExport.WriteAsync(PathFor(), ["id"], []));
        Assert.Equal("\"id\"\r\n", await File.ReadAllTextAsync(PathFor()));
        Assert.Throws<ArgumentException>(() => CsvExportRange.Create(0, CsvExportScope.Range, 0, 100, 1, 1));
    }

    [Fact]
    public async Task Source_failure_does_not_leave_a_partial_csv_or_temporary_file()
    {
        IEnumerable<SqlValue[]> Rows()
        {
            yield return [SqlValue.Number("1")];
            throw new IOException("read failed");
        }
        await Assert.ThrowsAsync<IOException>(() => CsvExport.WriteAsync(PathFor(), ["id"], Rows()));
        Assert.Empty(directory.EnumerateFiles());
    }

    [Fact]
    public async Task Existing_files_are_never_overwritten_and_temporary_files_are_removed()
    {
        await File.WriteAllTextAsync(PathFor(), "keep");
        await Assert.ThrowsAsync<IOException>(() => CsvExport.WriteAsync(PathFor(), ["id"], [[SqlValue.Number("1")]]));
        Assert.Equal("keep", await File.ReadAllTextAsync(PathFor()));
        Assert.Single(directory.EnumerateFiles());
    }

    [Fact]
    public async Task Cache_range_validation_rejects_overflow_and_disposed_results()
    {
        async IAsyncEnumerable<SqlResult> Batches()
        {
            await Task.CompletedTask;
            yield return new(["id"], [[SqlValue.Number("1")]], "");
        }
        var loaded = await LoadedQueryResult.LoadAsync(Batches());
        Assert.Throws<ArgumentOutOfRangeException>(() => loaded.ReadRows(-1, 1));
        Assert.Throws<ArgumentOutOfRangeException>(() => loaded.ReadRows(0, long.MaxValue));
        Assert.Throws<ArgumentOutOfRangeException>(() => loaded.ReadRows(long.MaxValue, 1));
        Assert.Empty(loaded.ReadRows(1, 0));
        loaded.Dispose();
        Assert.Throws<ObjectDisposedException>(() => loaded.ReadRows(0, 0));
    }

    public void Dispose() => directory.Delete(recursive: true);
}
