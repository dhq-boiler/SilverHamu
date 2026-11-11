using System.Text;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed class LoadedQueryResult : IDisposable
{
    public const int PageSize = 100;
    private readonly FileStream file;
    private readonly List<long> pages = [];
    private bool disposed;
    private SqlResult header = new([], [], "");
    public long RowCount { get; private set; }
    public long PageCount => Math.Max(1, pages.Count);

    private LoadedQueryResult()
    {
        var path = Path.Combine(Path.GetTempPath(), $"silverhamu-result-{Guid.NewGuid():N}.tmp");
        file = new(path, FileMode.CreateNew, FileAccess.ReadWrite, FileShare.None, 65536,
            FileOptions.DeleteOnClose);
    }

    public static async Task<LoadedQueryResult> LoadAsync(IAsyncEnumerable<SqlResult> batches)
    {
        var result = new LoadedQueryResult();
        try
        {
            using var writer = new BinaryWriter(result.file, Encoding.UTF8, leaveOpen: true);
            await foreach (var batch in batches)
            {
                result.header = batch with { Rows = [] };
                foreach (var row in batch.Rows)
                {
                    if (result.RowCount % PageSize == 0) result.pages.Add(result.file.Position);
                    foreach (var value in row)
                    {
                        writer.Write(value.Kind);
                        writer.Write(value.Text is not null);
                        if (value.Text is not null) writer.Write(value.Text);
                    }
                    result.RowCount++;
                }
            }
            writer.Flush();
            return result;
        }
        catch
        {
            result.Dispose();
            throw;
        }
    }

    public SqlResult ReadPage(long pageIndex)
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (pageIndex < 0 || pageIndex >= PageCount) throw new ArgumentOutOfRangeException(nameof(pageIndex));
        var count = (int)Math.Min(PageSize, RowCount - pageIndex * PageSize);
        var rows = new List<SqlValue[]>(count);
        if (count > 0)
        {
            file.Position = pages[(int)pageIndex];
            using var reader = new BinaryReader(file, Encoding.UTF8, leaveOpen: true);
            for (int i = 0; i < count; i++)
            {
                var row = new SqlValue[header.Columns.Count];
                for (int column = 0; column < row.Length; column++)
                    row[column] = new(reader.ReadString(), reader.ReadBoolean() ? reader.ReadString() : null);
                rows.Add(row);
            }
        }
        return header with { Rows = rows, Message = L.Get("AllNRowsAreCached", RowCount) };
    }

    public IEnumerable<SqlValue[]> ReadRows(long start, long count)
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (start < 0 || count < 0 || start > RowCount || count > RowCount - start)
            throw new ArgumentOutOfRangeException(nameof(start));
        return Read();

        IEnumerable<SqlValue[]> Read()
        {
            long position = start, remaining = count;
            while (remaining > 0)
            {
                var page = ReadPage(position / PageSize);
                int offset = (int)(position % PageSize);
                int take = (int)Math.Min(page.Rows.Count - offset, remaining);
                foreach (var row in page.Rows.Skip(offset).Take(take)) yield return row;
                position += take; remaining -= take;
            }
        }
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        file.Dispose();
    }
}
