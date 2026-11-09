using System.Globalization;
using System.Numerics;

namespace Catdb.Sql;

public sealed record SqlWindow(string Function, SqlExpression? Argument, List<SqlField> Partition,
    List<SqlOrder> Order, SqlRowFrame? Frame)
{
    public int Distance { get; init; } = 1;
    public SqlValue? Fallback { get; init; }
    public SqlExpression? Filter { get; init; }
}
public enum SqlFrameUnit { Rows, Groups, Range }
// Null represents an unbounded end. Finite bounds are relative to the current row, peer group or sort key.
public sealed record SqlRowFrame(decimal? Start, decimal? End, SqlFrameUnit Unit = SqlFrameUnit.Rows)
{
    internal bool HasDistance { get; init; } = Start.GetValueOrDefault() != 0 || End.GetValueOrDefault() != 0;
}

public sealed partial class SqlParser
{
    private enum FrameBoundKind { UnboundedPreceding, Preceding, Current, Following, UnboundedFollowing }

    private bool WindowProjectionStart()
    {
        if (Peek.Kind != "word" || current + 1 >= tokens.Count || tokens[current + 1].Text != "(")
            return false;
        var nesting = 0;
        for (int i = current + 1; i < tokens.Count - 1; i++)
        {
            if (tokens[i].Kind != "symbol") continue;
            if (tokens[i].Text == "(") nesting++;
            if (tokens[i].Text == ")" && --nesting == 0)
            {
                var next = tokens[i + 1];
                if (next.Kind == "word" && next.Text.Equals("FILTER", StringComparison.OrdinalIgnoreCase)
                    && i + 2 < tokens.Count && tokens[i + 2].Text == "(") { i++; continue; }
                return next.Kind == "word" && next.Text.Equals("OVER", StringComparison.OrdinalIgnoreCase);
            }
        }
        return false;
    }

    private SqlProjection ParseWindowProjection()
    {
        var function = Name().ToUpperInvariant();
        Expect("(");
        SqlExpression? argument = null;
        var distance = 1;
        SqlValue? fallback = null;
        if (function is "LAG" or "LEAD")
        {
            argument = Expression();
            if (Match(","))
            {
                distance = Integer(0, int.MaxValue);
                if (Match(",")) fallback = Literal();
            }
        }
        else if (function == "NTILE") distance = Integer(1, int.MaxValue);
        else if (function is "SUM" or "AVG" or "MIN" or "MAX" or "FIRST_VALUE" or "LAST_VALUE" ||
                 function == "COUNT" && !Match("*"))
            argument = Expression();
        else if (function is not ("ROW_NUMBER" or "RANK" or "DENSE_RANK" or "COUNT"))
            throw Error($"Unknown window function '{function}'.");
        Expect(")");
        var filter = AggregateFilter(0);
        if (filter is not null && function is not ("SUM" or "AVG" or "MIN" or "MAX" or "COUNT"))
            throw Error("FILTER requires an aggregate window function.");
        Expect("OVER"); Expect("(");
        var partition = new List<SqlField>();
        if (Match("PARTITION"))
        {
            Expect("BY");
            do { partition.Add(Field()); } while (Match(","));
        }
        var order = new List<SqlOrder>();
        if (Match("ORDER"))
        {
            Expect("BY");
            do { order.Add(OrderTerm()); } while (Match(","));
        }
        SqlRowFrame? frame = null;
        if (Is("ROWS") || Is("RANGE") || Is("GROUPS"))
        {
            var unit = Enum.Parse<SqlFrameUnit>(tokens[current++].Text, ignoreCase: true);
            var between = Match("BETWEEN");
            var start = WindowBound(unit, start: true, out var startKind);
            decimal? end = 0;
            var endKind = FrameBoundKind.Current;
            if (between) { Expect("AND"); end = WindowBound(unit, start: false, out endKind); }
            if (startKind > endKind) throw Error("The window frame starts after its end.");
            if (unit == SqlFrameUnit.Groups && order.Count == 0) throw Error("GROUPS requires ORDER BY.");
            bool hasDistance = startKind is FrameBoundKind.Preceding or FrameBoundKind.Following ||
                endKind is FrameBoundKind.Preceding or FrameBoundKind.Following;
            if (unit == SqlFrameUnit.Range && hasDistance && order.Count != 1)
                throw Error("RANGE with a distance requires exactly one ORDER BY column.");
            frame = new(start, end, unit) { HasDistance = hasDistance };
        }
        Expect(")");
        return new(null, Match("AS") ? Name() : function.ToLowerInvariant())
        {
            Window = new(function, argument, partition, order, frame) { Distance = distance, Fallback = fallback, Filter = filter }
        };
    }

    private decimal? WindowBound(SqlFrameUnit unit, bool start, out FrameBoundKind kind)
    {
        if (Match("UNBOUNDED"))
        {
            Expect(start ? "PRECEDING" : "FOLLOWING");
            kind = start ? FrameBoundKind.UnboundedPreceding : FrameBoundKind.UnboundedFollowing;
            return null;
        }
        if (Match("CURRENT")) { Expect("ROW"); kind = FrameBoundKind.Current; return 0; }
        decimal distance;
        if (unit != SqlFrameUnit.Range) distance = Integer(0, int.MaxValue);
        else if (Peek.Kind == "number") distance = SqlNumbers.Numeric(Literal());
        else throw Error("A RANGE distance must be a nonnegative numeric literal.");
        if (Match("PRECEDING")) { kind = FrameBoundKind.Preceding; return -distance; }
        Expect("FOLLOWING");
        kind = FrameBoundKind.Following;
        return distance;
    }
}

public sealed partial class SqlDatabase
{
    private sealed partial class SelectScope
    {
        private SqlWindow? BindWindow(SqlWindow? window) => window is null ? null : window with
        {
            Argument = BindExpression(window.Argument),
            Filter = BindExpression(window.Filter),
            Partition = window.Partition.Select(f => new SqlField(SelectKey(Resolve(f)))).ToList(),
            Order = window.Order.Select(o => o with { Column = SelectKey(Resolve(new(o.Column, o.Qualifier))), Qualifier = null }).ToList()
        };
    }

    private static string WindowResultType(SqlWindow window, IReadOnlyList<SqlColumn> columns)
    {
        if (window.Filter is { } filter) RequireType(ExpressionType(filter, columns), "BOOLEAN");
        if (window.Frame is { Unit: SqlFrameUnit.Range, HasDistance: true })
            RequireNumber(ExpressionType(new SqlField(window.Order.Single().Column), columns));
        if (window.Argument is null) return "INTEGER";
        var type = ExpressionType(window.Argument, columns);
        if (window.Fallback is { } fallback)
            type = CommonType(type, ExpressionType(new SqlLiteral(fallback), columns));
        if (window.Function is "SUM" or "AVG") RequireNumber(type);
        return window.Function switch { "COUNT" => "INTEGER", "AVG" or "SUM" => "REAL", _ => type };
    }

    private static SqlValue[] WindowValues(SqlWindow window, List<SqlRow> rows, OperatorTiming? timing)
    {
        long started = timing?.Start() ?? 0;
        var result = new SqlValue[rows.Count];
        var partitions = Enumerable.Range(0, rows.Count).GroupBy(i =>
            window.Partition.Select(f => rows[i].Values[f.Name]).ToArray(), new GroupKeyComparer());
        int CompareOrder(int a, int b)
        {
            foreach (var order in window.Order)
            {
                var comparison = order.Compare(rows[a].Values[order.Column], rows[b].Values[order.Column]);
                if (comparison != 0) return comparison;
            }
            return 0;
        }

        foreach (var partition in partitions)
        {
            // A stable order also keeps ROWS and ROW_NUMBER consistent across tied keys.
            var indices = partition.Order(Comparer<int>.Create(CompareOrder)).ToArray();
            var values = indices.Select(i => window.Filter?.Evaluate(rows[i].Values).IsTrue == false
                ? SqlValue.Null : window.Argument?.Evaluate(rows[i].Values) ?? SqlValue.Boolean(true)).ToArray();
            var aggregate = new WindowAccumulator(window.Function);
            var peers = new List<int> { 0 };
            for (int j = 1; j < indices.Length; j++)
                if (CompareOrder(indices[j - 1], indices[j]) != 0) peers.Add(j);
            peers.Add(indices.Length);
            BigInteger?[]? rangeKeys = window.Frame is { Unit: SqlFrameUnit.Range, HasDistance: true }
                ? indices.Select(row => rows[row].Values[window.Order[0].Column] is { IsNull: false } key
                    ? (BigInteger?)SqlNumbers.UnitsAtScale28(SqlNumbers.Numeric(key)) : null).ToArray() : null;
            int start = 0, end = 0, peer = 0;
            for (int i = 0; i < indices.Length; i++)
            {
                if (i == peers[peer + 1]) peer++;
                if (window.Function is "ROW_NUMBER" or "RANK" or "DENSE_RANK")
                {
                    var number = window.Function switch { "RANK" => peers[peer] + 1, "DENSE_RANK" => peer + 1, _ => i + 1 };
                    result[indices[i]] = SqlValue.Number(number.ToString(CultureInfo.InvariantCulture));
                    continue;
                }
                if (window.Function == "NTILE")
                {
                    // Larger buckets come first. When there are more buckets than rows,
                    // each occupied bucket has exactly one row.
                    int size = indices.Length / window.Distance, extra = indices.Length % window.Distance;
                    long boundary = (long)extra * (size + 1L);
                    long bucket = i < boundary ? i / (size + 1L) : extra + (i - boundary) / size;
                    result[indices[i]] = SqlNumbers.Number(bucket + 1);
                    continue;
                }
                if (window.Function is "LAG" or "LEAD")
                {
                    long index = (long)i + (window.Function == "LEAD" ? window.Distance : -window.Distance);
                    result[indices[i]] = index >= 0 && index < indices.Length ? values[index] : window.Fallback ?? SqlValue.Null;
                    continue;
                }
                var frame = window.Frame;
                int first = frame?.Start is decimal lo ? FramePosition(lo, false) : 0;
                int after = frame is null ? peers[peer + 1] : frame.End is decimal hi ? FramePosition(hi, true) : indices.Length;
                int FramePosition(decimal offset, bool afterEnd)
                {
                    int advance = afterEnd ? 1 : 0;
                    if (frame!.Unit == SqlFrameUnit.Rows)
                        return (int)Math.Clamp((long)i + (long)offset + advance, 0, indices.Length);
                    if (frame.Unit == SqlFrameUnit.Groups)
                        return peers[(int)Math.Clamp((long)peer + (long)offset + advance, 0, peers.Count - 1)];
                    if (offset == 0 || rangeKeys![i] is not { } key) return peers[peer + advance];
                    return RangePosition(rangeKeys, key, window.Order[0], offset, afterEnd);
                }
                if (window.Function is "FIRST_VALUE" or "LAST_VALUE")
                {
                    result[indices[i]] = first >= after ? SqlValue.Null : values[window.Function == "FIRST_VALUE" ? first : after - 1];
                    continue;
                }
                while (start < first && start < end) aggregate.Change(values[start++], -1);
                start = first;
                end = Math.Max(end, start);
                while (end < after) aggregate.Change(values[end++], 1);
                result[indices[i]] = aggregate.Value();
            }
        }
        timing?.End(started, result.Length);
        return result;
    }

    private static int RangePosition(BigInteger?[] keys, BigInteger current, SqlOrder order, decimal offset, bool afterEnd)
    {
        // Decimal addition would round a fractional boundary next to a 29-digit key.
        var target = current + SqlNumbers.UnitsAtScale28(order.Descending ? -offset : offset);
        int first = 0, last = keys.Length;
        while (first < last)
        {
            int middle = first + (last - first) / 2;
            int comparison;
            if (keys[middle] is not { } candidate) comparison = (order.NullsFirst ?? !order.Descending) ? -1 : 1;
            else
            {
                comparison = candidate.CompareTo(target);
                if (order.Descending) comparison = -comparison;
            }
            if (comparison < 0 || comparison == 0 && afterEnd) first = middle + 1;
            else last = middle;
        }
        return first;
    }

    private sealed class WindowAccumulator(string function)
    {
        private readonly SortedSet<SqlValue> ordered = new(Comparer<SqlValue>.Create(SqlValue.Compare));
        private readonly Dictionary<string, int> frequency = new(StringComparer.Ordinal);
        private int count;
        private decimal sum;

        public void Change(SqlValue value, int direction)
        {
            if (value.IsNull) return;
            count += direction;
            if (function is "SUM" or "AVG")
            {
                try { sum += direction * decimal.Parse(value.Text!, CultureInfo.InvariantCulture); }
                catch (OverflowException) { throw new SqlException("Window sum exceeds the decimal range."); }
            }
            if (function is "MIN" or "MAX")
            {
                var key = UniqueIdentity(value);
                frequency.TryGetValue(key, out var previous);
                if (previous + direction == 0) { frequency.Remove(key); ordered.Remove(value); }
                else { frequency[key] = previous + direction; ordered.Add(value); }
            }
        }
        public SqlValue Value()
        {
            if (function == "COUNT") return SqlValue.Number(count.ToString(CultureInfo.InvariantCulture));
            if (count == 0) return SqlValue.Null;
            if (function == "MIN") return ordered.Min!;
            if (function == "MAX") return ordered.Max!;
            return SqlValue.Number((function == "AVG" ? sum / count : sum).ToString(CultureInfo.InvariantCulture));
        }
    }
}
