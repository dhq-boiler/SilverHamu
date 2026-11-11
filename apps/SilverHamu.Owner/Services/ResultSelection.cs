using System.Text;

namespace SilverHamu.Owner.Services;

internal readonly record struct ResultAddress(int Row, int Column);

internal sealed class ResultSelection
{
    private readonly List<Area> areas = [];
    private ResultAddress? anchor;
    public ResultAddress? Focus { get; private set; }
    public int RowCount { get; private set; }
    public int ColumnCount { get; private set; }
    public long Count => areas.Sum(a => (long)(a.Bottom - a.Top + 1) * (a.Right - a.Left + 1));

    public void Reset(int rows, int columns)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(rows);
        ArgumentOutOfRangeException.ThrowIfNegative(columns);
        RowCount = rows; ColumnCount = columns;
        Clear();
    }

    public void Clear() { areas.Clear(); anchor = Focus = null; }
    public bool Contains(int row, int column) => areas.Any(a => row >= a.Top && row <= a.Bottom && column >= a.Left && column <= a.Right);
    public IEnumerable<int> SelectedRows() => areas.SelectMany(a => Enumerable.Range(a.Top, a.Bottom - a.Top + 1)).Distinct().Order();
    public bool ContainsRow(int row) => ColumnCount > 0 && areas.Where(a => row >= a.Top && row <= a.Bottom).Sum(a => (long)a.Right - a.Left + 1) == ColumnCount;

    public void SelectCell(int row, int column, bool extend = false, bool toggle = false)
    {
        Validate(row, column);
        var start = extend && anchor is { } old ? old with { Column = Math.Max(0, old.Column) } : new ResultAddress(row, column);
        var area = new Area(Math.Min(start.Row, row), Math.Max(start.Row, row), Math.Min(start.Column, column), Math.Max(start.Column, column));
        if (!extend) anchor = new(row, column);
        Focus = new(row, column);
        if (!extend && toggle && Contains(row, column)) Remove(area);
        else { if (!toggle) areas.Clear(); Add(area); }
    }

    public void SelectRow(int row, bool extend = false, bool toggle = false)
    {
        Validate(row, 0);
        int start = extend && anchor is { } old ? old.Row : row;
        var area = new Area(Math.Min(start, row), Math.Max(start, row), 0, ColumnCount - 1);
        if (!extend) anchor = new(row, -1);
        Focus = new(row, -1);
        if (!extend && toggle && ContainsRow(row)) Remove(area);
        else { if (!toggle) areas.Clear(); Add(area); }
    }

    public void SelectAll()
    {
        Clear();
        if (RowCount == 0 || ColumnCount == 0) return;
        areas.Add(new(0, RowCount - 1, 0, ColumnCount - 1));
        anchor = Focus = new(0, 0);
    }

    private void Validate(int row, int column)
    {
        if (row < 0 || row >= RowCount) throw new ArgumentOutOfRangeException(nameof(row));
        if (column < 0 || column >= ColumnCount) throw new ArgumentOutOfRangeException(nameof(column));
    }

    private void Add(Area area)
    {
        // 全行選択をセルごとの集合へ展開すると、大きな結果で選択だけに大量のメモリを使う。
        var pending = new List<Area> { area };
        foreach (var existing in areas) pending = pending.SelectMany(a => a.Subtract(existing)).ToList();
        areas.AddRange(pending);
    }

    private void Remove(Area area)
    {
        var remaining = areas.SelectMany(a => a.Subtract(area)).ToArray();
        areas.Clear(); areas.AddRange(remaining);
    }

    public string CopyText(IReadOnlyList<string[]> rows, IReadOnlyList<string> columns, bool includeHeaders = false)
    {
        if (areas.Count == 0) return "";
        if (rows.Count != RowCount || columns.Count != ColumnCount) throw new ArgumentException("The result changed after selection.");
        if (Count == 1 && !includeHeaders) return rows[areas[0].Top][areas[0].Left];
        int firstColumn = areas.Min(a => a.Left), lastColumn = areas.Max(a => a.Right);
        var text = new StringBuilder();
        bool wroteRow = false;
        void AppendRow(Func<int, string> value)
        {
            if (wroteRow) text.Append("\r\n");
            wroteRow = true;
            for (int column = firstColumn; column <= lastColumn; column++)
            {
                if (column > firstColumn) text.Append('\t');
                var cell = value(column);
                if (cell.IndexOfAny(['\t', '\r', '\n', '"']) >= 0) text.Append('"').Append(cell.Replace("\"", "\"\"")).Append('"');
                else text.Append(cell);
            }
        }
        if (includeHeaders) AppendRow(column => columns[column]);
        int lastRow = areas.Max(a => a.Bottom);
        for (int row = areas.Min(a => a.Top); row <= lastRow; row++)
        {
            if (!areas.Any(a => row >= a.Top && row <= a.Bottom)) continue;
            int index = row;
            AppendRow(column => Contains(index, column) ? rows[index][column] : "");
        }
        return text.ToString();
    }

    private readonly record struct Area(int Top, int Bottom, int Left, int Right)
    {
        public IEnumerable<Area> Subtract(Area other)
        {
            int top = Math.Max(Top, other.Top), bottom = Math.Min(Bottom, other.Bottom);
            int left = Math.Max(Left, other.Left), right = Math.Min(Right, other.Right);
            if (top > bottom || left > right) { yield return this; yield break; }
            if (Top < top) yield return new(Top, top - 1, Left, Right);
            if (bottom < Bottom) yield return new(bottom + 1, Bottom, Left, Right);
            if (Left < left) yield return new(top, bottom, Left, left - 1);
            if (right < Right) yield return new(top, bottom, right + 1, Right);
        }
    }
}
