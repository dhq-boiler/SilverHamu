using System.Text;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal enum CsvExportScope { All, CurrentPage, Range }

internal sealed record CsvExportRange(long Start, long Count)
{
    public static CsvExportRange Create(long total, CsvExportScope scope, long page, int pageSize,
        long first = 1, long last = 0)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(total);
        ArgumentOutOfRangeException.ThrowIfNegativeOrZero(pageSize);
        if (scope == CsvExportScope.All) return new(0, total);
        if (scope == CsvExportScope.CurrentPage)
        {
            if (page < 0 || page > (total == 0 ? 0 : (total - 1) / pageSize))
                throw new ArgumentException(L.Get("ThePageIsOutsideTheCachedResultRange"));
            long start = page * pageSize;
            return new(start, Math.Min(pageSize, total - start));
        }
        if (scope != CsvExportScope.Range) throw new ArgumentOutOfRangeException(nameof(scope));
        if (first < 1 || last < first || last > total)
            throw new ArgumentException(L.Get("EnterFirstAndLastRowsFromToN", total));
        return new(first - 1, last - first + 1);
    }
}

internal static class CsvExport
{
    public static async Task<long> WriteAsync(string destination, IReadOnlyList<string> columns,
        IEnumerable<SqlValue[]> rows)
    {
        var path = Path.GetFullPath(destination);
        var temporary = Path.Combine(Path.GetDirectoryName(path)!, $".{Guid.NewGuid():N}.csv.tmp");
        long count = 0;
        try
        {
            await using (var file = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None, 65536, true))
            await using (var writer = new StreamWriter(file, new UTF8Encoding(true), 65536) { NewLine = "\r\n" })
            {
                await writer.WriteLineAsync(string.Join(',', columns.Select(Quote)));
                foreach (var row in rows)
                {
                    await writer.WriteLineAsync(string.Join(',', row.Select(value => Quote(value.Display))));
                    count++;
                }
            }
            File.Move(temporary, path, overwrite: false);
            return count;
        }
        finally { File.Delete(temporary); }
    }

    private static string Quote(string value) => "\"" + value.Replace("\"", "\"\"") + "\"";
}
