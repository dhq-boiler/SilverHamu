using System.Globalization;

namespace Catdb.Sql;

public sealed record SqlWindow(string Function, SqlExpression? Argument, List<SqlField> Partition,
    List<SqlOrder> Order, SqlRowFrame? Frame)
{
    public int Distance { get; init; } = 1;
    public SqlValue? Fallback { get; init; }
}
// Null represents an unbounded end. Finite bounds are relative to the current row.
public sealed record SqlRowFrame(int? Start, int? End);

public sealed partial class SqlParser
{
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
                return tokens[i + 1].Kind == "word" && tokens[i + 1].Text.Equals("OVER", StringComparison.OrdinalIgnoreCase);
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
        Expect(")"); Expect("OVER"); Expect("(");
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
            do
            {
                var field = Field();
                var descending = Match("DESC");
                if (!descending) Match("ASC");
                order.Add(new(field.Name, descending, field.Qualifier));
            } while (Match(","));
        }
        SqlRowFrame? frame = null;
        if (Match("ROWS"))
        {
            var between = Match("BETWEEN");
            var start = WindowBound(start: true);
            int? end = 0;
            if (between) { Expect("AND"); end = WindowBound(start: false); }
            if (start > end) throw Error("The window frame starts after its end.");
            frame = new(start, end);
        }
        Expect(")");
        return new(null, Match("AS") ? Name() : function.ToLowerInvariant())
        {
            Window = new(function, argument, partition, order, frame) { Distance = distance, Fallback = fallback }
        };
    }

    private int? WindowBound(bool start)
    {
        if (Match("UNBOUNDED")) { Expect(start ? "PRECEDING" : "FOLLOWING"); return null; }
        if (Match("CURRENT")) { Expect("ROW"); return 0; }
        var distance = Integer(0, int.MaxValue);
        if (Match("PRECEDING")) return -distance;
        Expect("FOLLOWING");
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
            Partition = window.Partition.Select(f => new SqlField(SelectKey(Resolve(f)))).ToList(),
            Order = window.Order.Select(o => new SqlOrder(SelectKey(Resolve(new(o.Column, o.Qualifier))), o.Descending)).ToList()
        };
    }

    private static string WindowResultType(SqlWindow window, IReadOnlyList<SqlColumn> columns)
    {
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
                var comparison = SqlValue.Compare(rows[a].Values[order.Column], rows[b].Values[order.Column]);
                if (comparison != 0) return order.Descending ? -comparison : comparison;
            }
            return 0;
        }

        foreach (var partition in partitions)
        {
            // A stable order also keeps ROWS and ROW_NUMBER consistent across tied keys.
            var indices = partition.Order(Comparer<int>.Create(CompareOrder)).ToArray();
            var values = indices.Select(i => window.Argument?.Evaluate(rows[i].Values) ?? SqlValue.Boolean(true)).ToArray();
            var aggregate = new WindowAccumulator(window.Function);
            int start = 0, end = 0, rank = 0, denseRank = 0, peerEnd = 0;
            for (int i = 0; i < indices.Length; i++)
            {
                if (i == peerEnd)
                {
                    rank = i + 1;
                    denseRank++;
                    peerEnd++;
                    while (peerEnd < indices.Length && CompareOrder(indices[i], indices[peerEnd]) == 0) peerEnd++;
                }
                if (window.Function is "ROW_NUMBER" or "RANK" or "DENSE_RANK")
                {
                    var number = window.Function switch { "RANK" => rank, "DENSE_RANK" => denseRank, _ => i + 1 };
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
                int first = frame?.Start is int lo ? (int)Math.Clamp((long)i + lo, 0, indices.Length) : 0;
                int after = frame is null ? peerEnd : frame.End is int hi
                    ? (int)Math.Clamp((long)i + hi + 1, 0, indices.Length) : indices.Length;
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
