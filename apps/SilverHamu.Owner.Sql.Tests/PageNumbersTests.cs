using System.Collections;
using SilverHamu.Owner.Services;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class PageNumbersTests
{
    [Theory]
    [InlineData(1)]
    [InlineData(3)]
    [InlineData(10)]
    [InlineData(11)]
    [InlineData(30)]
    [InlineData(31)]
    public void Page_numbers_are_continuous_and_stop_at_the_actual_last_page(int count)
    {
        var pages = new PageNumbers(count);
        Assert.Equal(count, pages.Count);
        Assert.Equal(Enumerable.Range(1, count), pages);
        Assert.Equal(count, pages[count - 1]);
        Assert.Throws<ArgumentOutOfRangeException>(() => pages[-1]);
        Assert.Throws<ArgumentOutOfRangeException>(() => pages[count]);
    }

    [Fact]
    public void Virtualized_number_source_supports_very_large_page_counts_without_an_array()
    {
        var pages = new PageNumbers(int.MaxValue);
        Assert.Equal(1, pages[0]);
        Assert.Equal(10, pages[9]);
        Assert.Equal(11, pages[10]);
        Assert.Equal(21, pages[20]);
        Assert.Equal(int.MaxValue, pages[int.MaxValue - 1]);
        IList source = pages;
        Assert.Equal(21, source[20]);
        Assert.True(source.Contains(int.MaxValue));
        Assert.Equal(int.MaxValue - 1, source.IndexOf(int.MaxValue));
        Assert.Equal(-1, source.IndexOf(0));
        Assert.Throws<NotSupportedException>(() => source[0] = 9);
    }

    [Theory]
    [InlineData("1", 3, 0)]
    [InlineData("3", 3, 2)]
    [InlineData(" 2 ", 3, 1)]
    [InlineData("210", 210, 209)]
    [InlineData("2147483647", 2147483647, 2147483646)]
    public void Direct_input_resolves_to_the_requested_cached_page(string text, long count, long expected) =>
        Assert.Equal(expected, PageNumbers.Parse(text, count));

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("0")]
    [InlineData("-1")]
    [InlineData("4")]
    [InlineData("1.5")]
    [InlineData("abc")]
    [InlineData("9223372036854775808")]
    public void Invalid_input_does_not_resolve_to_any_page(string? text) =>
        Assert.Throws<ArgumentException>(() => PageNumbers.Parse(text, 3));
}
