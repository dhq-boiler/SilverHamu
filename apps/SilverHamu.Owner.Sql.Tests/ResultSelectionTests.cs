using SilverHamu.Owner.Services;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ResultSelectionTests
{
    private static readonly string[] Columns = ["Id", "Title", "Note"];
    private static readonly string[][] Rows = [["1", "猫", "A"], ["2", "犬", "B"], ["3", "鳥", "C"]];
    private static ResultSelection Create(int rows = 3, int columns = 3) { var selection = new ResultSelection(); selection.Reset(rows, columns); return selection; }

    [Theory]
    [InlineData(0, 0)]
    [InlineData(0, 2)]
    [InlineData(2, 0)]
    [InlineData(2, 2)]
    public void A_cell_at_each_edge_copies_its_value_without_row_numbers(int row, int column)
    {
        var selection = Create(); selection.SelectCell(row, column);
        Assert.Equal(1, selection.Count);
        Assert.Equal(Rows[row][column], selection.CopyText(Rows, Columns));
    }

    [Theory]
    [InlineData(0, 0, 2, 2)]
    [InlineData(2, 2, 0, 0)]
    [InlineData(2, 0, 0, 2)]
    [InlineData(0, 2, 2, 0)]
    public void Shift_selects_an_inclusive_rectangle_in_any_direction(int startRow, int startColumn, int endRow, int endColumn)
    {
        var selection = Create(); selection.SelectCell(startRow, startColumn); selection.SelectCell(endRow, endColumn, extend: true);
        Assert.Equal(9, selection.Count);
        Assert.Equal("1\t猫\tA\r\n2\t犬\tB\r\n3\t鳥\tC", selection.CopyText(Rows, Columns));
    }

    [Fact]
    public void Ctrl_adds_and_removes_cells_and_keeps_sparse_column_positions()
    {
        var selection = Create(); selection.SelectCell(2, 2); selection.SelectCell(0, 0, toggle: true);
        Assert.Equal("1\t\t\r\n\t\tC", selection.CopyText(Rows, Columns));
        selection.SelectCell(2, 2, toggle: true);
        Assert.Equal("1", selection.CopyText(Rows, Columns));
    }

    [Fact]
    public void Row_headers_select_all_columns_and_ctrl_skips_unselected_rows()
    {
        var selection = Create(); selection.SelectRow(2); selection.SelectRow(0, toggle: true);
        Assert.Equal(6, selection.Count);
        Assert.Equal("1\t猫\tA\r\n3\t鳥\tC", selection.CopyText(Rows, Columns));
        Assert.True(selection.ContainsRow(0)); Assert.False(selection.ContainsRow(1));
        selection.SelectRow(2, toggle: true);
        Assert.Equal(3, selection.Count);
    }

    [Fact]
    public void Shift_on_row_headers_includes_every_row_between_the_anchor_and_target()
    {
        var selection = Create(); selection.SelectRow(2); selection.SelectRow(0, extend: true);
        Assert.Equal(9, selection.Count);
        Assert.All(Enumerable.Range(0, 3), row => Assert.True(selection.ContainsRow(row)));
    }

    [Fact]
    public void Mixed_rows_cells_and_overlapping_ranges_do_not_duplicate_values()
    {
        var selection = Create(); selection.SelectRow(1); selection.SelectCell(0, 1, toggle: true);
        selection.SelectCell(2, 2, extend: true, toggle: true);
        Assert.Equal(7, selection.Count);
        Assert.Equal("\t猫\tA\r\n2\t犬\tB\r\n\t鳥\tC", selection.CopyText(Rows, Columns));
    }

    [Fact]
    public void A_cell_can_be_removed_from_the_middle_of_a_rectangle_and_added_back()
    {
        var selection = Create(); selection.SelectAll(); selection.SelectCell(1, 1, toggle: true);
        Assert.Equal(8, selection.Count);
        Assert.Equal("1\t猫\tA\r\n2\t\tB\r\n3\t鳥\tC", selection.CopyText(Rows, Columns));
        selection.SelectCell(1, 1, toggle: true);
        Assert.Equal(9, selection.Count);
        selection.SelectRow(1, toggle: true);
        Assert.Equal(6, selection.Count); Assert.False(selection.ContainsRow(1));
    }

    [Fact]
    public void Headers_are_optional_and_match_the_selected_columns()
    {
        var selection = Create(); selection.SelectCell(1, 1); selection.SelectCell(2, 2, extend: true);
        Assert.Equal("Title\tNote\r\n犬\tB\r\n鳥\tC", selection.CopyText(Rows, Columns, includeHeaders: true));
    }

    [Fact]
    public void Multi_cell_copy_quotes_tabs_newlines_and_quotes_without_changing_unicode()
    {
        var selection = Create(1, 3); selection.SelectAll();
        Assert.Equal("\"猫\t犬\"\t\"A\r\nB\"\t\"say \"\"hi\"\"\"", selection.CopyText([["猫\t犬", "A\r\nB", "say \"hi\""]], Columns));
    }

    [Fact]
    public void Single_cell_copy_preserves_the_raw_value_including_whitespace()
    {
        var selection = Create(1, 1); selection.SelectCell(0, 0);
        var value = " \"猫\"\t犬\n鳥 ";
        Assert.Equal(value, selection.CopyText([[value]], ["Value"]));
    }

    [Fact]
    public void Empty_first_rows_still_have_line_separators()
    {
        var selection = Create(2, 1); selection.SelectAll();
        Assert.Equal("\r\nsecond", selection.CopyText([[""], ["second"]], ["Value"]));
    }

    [Fact]
    public void Large_all_selection_uses_a_long_count_and_allows_a_single_cell_to_be_excluded()
    {
        var selection = Create(1_000_000, 10_000); selection.SelectAll();
        Assert.Equal(10_000_000_000L, selection.Count);
        selection.SelectCell(500_000, 5_000, toggle: true);
        Assert.Equal(9_999_999_999L, selection.Count);
        Assert.False(selection.Contains(500_000, 5_000)); Assert.True(selection.Contains(500_000, 5_001));
    }

    [Theory]
    [InlineData(0, 0)]
    [InlineData(0, 3)]
    [InlineData(3, 0)]
    public void Empty_results_do_not_select_or_copy_data(int rows, int columns)
    {
        var selection = Create(rows, columns); selection.SelectAll();
        Assert.Equal(0, selection.Count); Assert.Equal("", selection.CopyText([], []));
    }

    [Fact]
    public void Replacing_a_result_or_clearing_selection_removes_old_coordinates()
    {
        var selection = Create(); selection.SelectAll(); selection.Reset(1, 1);
        Assert.Equal(0, selection.Count); Assert.Null(selection.Focus);
        selection.SelectCell(0, 0); selection.Clear();
        Assert.Equal(0, selection.Count); Assert.False(selection.Contains(0, 0));
    }

    [Theory]
    [InlineData(-1, 0)]
    [InlineData(3, 0)]
    [InlineData(0, -1)]
    [InlineData(0, 3)]
    public void Coordinates_outside_the_current_result_are_rejected(int row, int column) =>
        Assert.Throws<ArgumentOutOfRangeException>(() => Create().SelectCell(row, column));

    [Fact]
    public void Copy_rejects_a_result_that_changed_after_selection()
    {
        var selection = Create(); selection.SelectAll();
        Assert.Throws<ArgumentException>(() => selection.CopyText([], Columns));
    }
}
