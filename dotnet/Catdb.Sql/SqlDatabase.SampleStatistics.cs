using System.Globalization;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    public static int ReadOnlySampleSize(SqlTable table, long totalRows)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(totalRows);
        if (table.Columns.Count == 0) throw new SqlException("統計の収集には列が必要です。");
        return (int)Math.Min(totalRows, Math.Min(1600, Math.Max(1, 65536 / table.Columns.Count)));
    }

    public static async Task<SqlTableStatistics> AnalyzeReadOnlySampleAsync(SqlTable table, long totalRows,
        IAsyncEnumerable<SqlValue[]> sample)
    {
        var expected = ReadOnlySampleSize(table, totalRows);
        var frequencies = table.Columns.Select(_ => new Dictionary<string, int>(StringComparer.Ordinal)).ToArray();
        int rows = 0;
        await foreach (var row in sample.ConfigureAwait(false))
        {
            if (++rows > expected || row.Length != frequencies.Length)
                throw new SqlException("統計のサンプルサイズまたは列数が定義と一致しません。");
            for (int i = 0; i < row.Length; i++)
            {
                if (row[i].IsNull) continue;
                var identity = StatisticsDigest(row[i]);
                var counts = frequencies[i];
                counts[identity] = counts.GetValueOrDefault(identity) + 1;
            }
        }
        if (rows != expected) throw new SqlException("統計の収集中に予定の行数を読み込めませんでした。");

        long Scale(int count) => rows == 0 ? 0 : (long)((decimal)totalRows * count / rows);
        var columns = new Dictionary<string, SqlColumnStatistics>(StringComparer.OrdinalIgnoreCase);
        for (int i = 0; i < frequencies.Length; i++)
        {
            var counts = frequencies[i];
            int singletons = counts.Values.Count(count => count == 1);
            long distinct = Scale(singletons) + (counts.Count - singletons);
            var frequent = counts.Where(pair => pair.Value > 1).OrderByDescending(pair => pair.Value)
                .ThenBy(pair => pair.Key, StringComparer.Ordinal).Take(8)
                .ToDictionary(pair => pair.Key, pair => Scale(pair.Value));
            columns.Add(table.Columns[i].Name, new(Scale(counts.Values.Sum()), distinct, frequent));
        }
        return new(totalRows, columns);
    }

    private static string StatisticsDigest(SqlValue value)
    {
        // Archive floating-point values can have exponents outside the SQL engine's decimal range.
        var text = value.Text!;
        if (value.Kind == "NUMBER" && decimal.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out var number))
            text = number.ToString("G29", CultureInfo.InvariantCulture);
        return Digest(text);
    }
}
