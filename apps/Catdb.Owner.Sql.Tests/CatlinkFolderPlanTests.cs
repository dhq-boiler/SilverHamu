using System.Buffers.Binary;
using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Catdb.Owner.Services;
using Catdb.Sql;
using NewDbms;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class CatlinkFolderPlanTests : IDisposable
{
    private const int RowCount = 103;
    private const string Day = "2026-11-10";
    private readonly DirectoryInfo root = Directory.CreateTempSubdirectory("catdb-folder-plans-");
    private string Data => Path.Combine(root.FullName, "data");
    private string Link => Path.Combine(root.FullName, "books.catlink");
    private string SqlFile => Path.Combine(Data, "books.ndb");
    private string Segment => Path.Combine(Data, "table-books.ndb");

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task Folder_plans_measure_Book_3_and_keep_statistics_only_for_the_open_session(bool palv)
    {
        await CreateFolder(palv);
        var original = SourceFiles();
        const string lookup = "SELECT Id FROM Book_3 WHERE PublishDate='2026-11-10'";
        decimal? initialFilter;
        using (var folder = await OpenFolder())
        {
            SqlResult initial;
            // The table data is inaccessible. A metadata-only plan must still succeed.
            using (File.Open(palv ? Segment : SqlFile, FileMode.Open, FileAccess.Read, FileShare.None))
                initial = await Execute(folder, "EXPLAIN " + lookup);
            initialFilter = Metric(initial, "FILTER", "estimated_rows");
            var analyze = await Execute(folder, "ANALYZE Book_3");
            Assert.Contains(RowCount.ToString(CultureInfo.InvariantCulture), analyze.Message);
            var plan = await Execute(folder, "EXPLAIN " + lookup);
            Assert.Equal(RowCount, Metric(plan, "ARCHIVE SCAN", "estimated_rows"));
            Assert.Equal(34m, Metric(plan, "FILTER", "estimated_rows"));

            const string rows = "SELECT Id FROM Book_3 WHERE PublishDate IS NOT NULL ORDER BY Id";
            var profile = await Execute(folder, "EXPLAIN ANALYZE " + rows);
            Assert.Equal(RowCount, Metric(profile, "ARCHIVE SCAN", "actual_rows"));
            Assert.Equal(34m, Metric(profile, "FILTER", "estimated_rows"));
            Assert.Equal(34m, Metric(profile, "FILTER", "actual_rows"));
            Assert.Equal(34m, Metric(profile, "RESULT", "actual_rows"));
            var visible = await Execute(folder, rows);
            Assert.Equal(Enumerable.Range(1, 34).Select(i => (3 * i).ToString(CultureInfo.InvariantCulture)),
                visible.Rows.Select(row => row[0].Text));
            var count = await Execute(folder, "SELECT COUNT(*) FROM Book_3");
            Assert.Equal("103", Assert.Single(Assert.Single(count.Rows)).Text);
            await Assert.ThrowsAsync<SqlException>(() => folder.ExecuteAsync("DELETE FROM Book_3"));
        }
        using (var reopened = await OpenFolder())
        {
            var plan = await Execute(reopened, "EXPLAIN " + lookup);
            Assert.Equal(initialFilter, Metric(plan, "FILTER", "estimated_rows"));
            Assert.NotEqual(34m, initialFilter);
        }
        Assert.Equal(original, SourceFiles());
    }

    [Fact]
    public async Task Statistics_from_an_earlier_scan_do_not_replace_the_actual_count_of_a_SQL_file()
    {
        await CreateFolder(palv: false);
        using var folder = await OpenFolder();
        await Execute(folder, "ANALYZE Book_3");
        using (var writer = await CatdbSqlConnection.OpenAsync(SqlFile))
            await writer.ExecuteAsync("INSERT INTO Book_3 VALUES (104,NULL)");
        var count = await Execute(folder, "SELECT COUNT(*) FROM Book_3");
        Assert.Equal("104", Assert.Single(Assert.Single(count.Rows)).Text);
    }

    private static async Task<SqlResult> Execute(CatlinkFolder folder, string statement) =>
        Assert.Single(await folder.ExecuteAsync(statement));

    private static decimal? Metric(SqlResult plan, string operation, string metric)
    {
        var fields = plan.Columns.ToList();
        var row = Assert.Single(plan.Rows, r => r[fields.IndexOf("operation")].Text == operation);
        var number = row[fields.IndexOf(metric)].Text;
        return number is null ? null : decimal.Parse(number, CultureInfo.InvariantCulture);
    }

    private Task<CatlinkFolder> OpenFolder() => CatlinkFolder.LoadAsync(Catlink.Resolve(Link));

    private async Task CreateFolder(bool palv)
    {
        Directory.CreateDirectory(Data);
        await File.WriteAllTextAsync(Link, "data");
        if (palv)
        {
            CreatePalv();
            return;
        }
        using var db = await CatdbSqlConnection.OpenAsync(SqlFile, create: true);
        var values = Enumerable.Range(1, RowCount)
            .Select(id => FormattableString.Invariant($"({id},{(id % 3 == 0 ? $"'{Day}'" : "NULL")})"));
        await db.ExecuteAsync("CREATE TABLE Book_3(Id INTEGER, PublishDate TEXT); INSERT INTO Book_3 VALUES " + string.Join(",", values));
    }

    private void CreatePalv()
    {
        using (var library = NewDbmsDatabase.Create(Path.Combine(Data, "library.ndb"), Guid.NewGuid()))
        using (var tx = library.BeginTransaction())
        {
            var table = new ArchiveTable("Book_3", [new("Id", "INTEGER"), new("PublishDate", "TEXT")], RowCount,
                [new(Path.GetFileName(Segment), 0, RowCount)]);
            tx.Put("palv/import/manifest/v1"u8.ToArray(), JsonSerializer.SerializeToUtf8Bytes(new ArchiveCatalog(1, "UTF-8", [table])));
            tx.Commit();
        }
        using var segment = NewDbmsDatabase.Create(Segment, Guid.NewGuid());
        using var transaction = segment.BeginTransaction();
        for (int id = 1; id <= RowCount; id++)
        {
            var key = new byte[13];
            key[0] = 1;
            BinaryPrimitives.WriteInt64BigEndian(key.AsSpan(5), id - 1);
            using var stream = new MemoryStream();
            using var row = new BinaryWriter(stream, Encoding.UTF8);
            row.Write(2);
            row.Write((byte)1);
            row.Write(sizeof(long));
            row.Write((long)id);
            bool hasDate = id % 3 == 0;
            var date = hasDate ? Encoding.UTF8.GetBytes(Day) : [];
            row.Write((byte)(hasDate ? 3 : 0));
            row.Write(date.Length);
            row.Write(date);
            transaction.Put(key, stream.ToArray());
        }
        transaction.Commit();
    }

    private (string Name, DateTime Modified, string Digest)[] SourceFiles() =>
        Directory.EnumerateFiles(root.FullName, "*", SearchOption.AllDirectories).Order(StringComparer.Ordinal).Select(path =>
        {
            using var input = File.OpenRead(path);
            return (Path.GetRelativePath(root.FullName, path), File.GetLastWriteTimeUtc(path), Convert.ToHexString(SHA256.HashData(input)));
        }).ToArray();

    public void Dispose() => root.Delete(recursive: true);
}
