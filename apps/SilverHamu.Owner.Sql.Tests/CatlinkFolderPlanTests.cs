using System.Buffers.Binary;
using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using NewDbms;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class CatlinkFolderPlanTests : IDisposable
{
    private const int RowCount = 103;
    private const string Day = "2026-11-10";
    private readonly DirectoryInfo root = Directory.CreateTempSubdirectory("silverhamu-folder-plans-");
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
            await Assert.ThrowsAsync<SqlException>(() => folder.ExecuteAsync("DROP TABLE Book_3"));
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
        using (var writer = await SilverHamuSqlConnection.OpenAsync(SqlFile))
            await writer.ExecuteAsync("INSERT INTO Book_3 VALUES (104,NULL)");
        var count = await Execute(folder, "SELECT COUNT(*) FROM Book_3");
        Assert.Equal("104", Assert.Single(Assert.Single(count.Rows)).Text);
    }

    [Fact]
    public async Task Book_archive_can_be_closed_after_loading_and_all_4201_rows_remain_pageable()
    {
        await CreateFolder(palv: true, count: 4201);
        var original = SourceFiles();
        LoadedQueryResult loaded;
        using (var folder = await OpenFolder())
            loaded = await folder.LoadQueryAsync(Assert.IsType<SelectRows>(Assert.Single(SqlParser.Parse("SELECT * FROM Book_3"))));
        using (loaded)
        {
            Assert.Equal(4201, loaded.RowCount);
            Assert.Equal(43, loaded.PageCount);
            Assert.Equal("4101", loaded.ReadPage(41).Rows[0][0].Display);
            Assert.Equal("4201", Assert.Single(loaded.ReadPage(42).Rows)[0].Display);
            Assert.Equal("1", loaded.ReadPage(0).Rows[0][0].Display);
        }
        Assert.Equal(original, SourceFiles());
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

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task Folder_updates_commit_and_rollback_without_losing_original_rows(bool palv)
    {
        await CreateFolder(palv);
        using (var folder = await OpenFolder())
        {
            await folder.ExecuteAsync("BEGIN; SAVEPOINT initial; UPDATE Book_3 SET PublishDate='pending' WHERE Id=1;");
            Assert.True(folder.InTransaction);
            Assert.Equal("pending", (await Execute(folder, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].Text);
            await folder.ExecuteAsync("ROLLBACK TO SAVEPOINT initial; RELEASE SAVEPOINT initial;");
            Assert.True((await Execute(folder, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].IsNull);
            await folder.ExecuteAsync("INSERT INTO Book_3 VALUES(104,'new'); DELETE FROM Book_3 WHERE Id=2; ROLLBACK;");
            Assert.False(folder.InTransaction);
            Assert.Equal("103", (await Execute(folder, "SELECT COUNT(*) FROM Book_3")).Rows.Single()[0].Text);
            await folder.ExecuteAsync("BEGIN; UPDATE Book_3 SET PublishDate='saved' WHERE Id=1; INSERT INTO Book_3 VALUES(104,NULL); DELETE FROM Book_3 WHERE Id=2; COMMIT;");
        }
        using var reopened = await OpenFolder();
        Assert.Equal("103", (await Execute(reopened, "SELECT COUNT(*) FROM Book_3")).Rows.Single()[0].Text);
        Assert.Equal("saved", (await Execute(reopened, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].Text);
        Assert.Empty((await Execute(reopened, "SELECT * FROM Book_3 WHERE Id=2")).Rows);
        Assert.Single((await Execute(reopened, "SELECT * FROM Book_3 WHERE Id=104")).Rows);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task Folder_autocommit_and_dispose_rollback_are_durable(bool palv)
    {
        await CreateFolder(palv);
        using (var folder = await OpenFolder())
        {
            await folder.ExecuteAsync("UPDATE Book_3 SET PublishDate='saved' WHERE Id=1;");
            await folder.ExecuteAsync("BEGIN; DELETE FROM Book_3;");
        }
        using var reopened = await OpenFolder();
        Assert.Equal("103", (await Execute(reopened, "SELECT COUNT(*) FROM Book_3")).Rows.Single()[0].Text);
        Assert.Equal("saved", (await Execute(reopened, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Archive_updates_and_edit_selection_include_rows_beyond_4096()
    {
        await CreateFolder(true, 4201);
        using var folder = await OpenFolder();
        Assert.Equal(4201, (await folder.ReadEditQueryAsync("SELECT * FROM Book_3")).Rows.Count);
        await folder.ExecuteAsync("UPDATE Book_3 SET PublishDate='last' WHERE Id=4201;");
        Assert.Equal("last", (await Execute(folder, "SELECT PublishDate FROM Book_3 WHERE Id=4201")).Rows.Single()[0].Text);
        Assert.Equal("4201", (await Execute(folder, "SELECT COUNT(*) FROM Book_3")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task SQL_folder_rejects_cross_file_writes_without_partial_commit()
    {
        await CreateFolder(false);
        using (var other = await SilverHamuSqlConnection.OpenAsync(Path.Combine(Data, "other.ndb"), create: true))
            await other.ExecuteAsync("CREATE TABLE Other(Id INTEGER); INSERT INTO Other VALUES(1);");
        using var folder = await OpenFolder();
        await folder.ExecuteAsync("BEGIN; DELETE FROM Book_3;");
        await Assert.ThrowsAsync<SqlException>(() => folder.ExecuteAsync("DELETE FROM Other;"));
        await folder.ExecuteAsync("ROLLBACK;");
        Assert.Equal("103", (await Execute(folder, "SELECT COUNT(*) FROM Book_3")).Rows.Single()[0].Text);
        Assert.Equal("1", (await Execute(folder, "SELECT COUNT(*) FROM Other")).Rows.Single()[0].Text);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task Folder_edit_batches_use_returning_and_rollback_conflicting_rows(bool palv)
    {
        await CreateFolder(palv);
        using var folder = await OpenFolder();
        var session = new TableEditSession(folder.Tables.Single().Schema,
            await folder.ReadEditQueryAsync("SELECT * FROM Book_3 ORDER BY Id;"), "SELECT * FROM Book_3 ORDER BY Id;", 1);
        var sql = session.Stage(0, 1, SqlValue.String("edited"));
        Assert.True(await TableEditBatch.ExecuteAsync(sql, "Book_3", folder.ExecuteAsync, () => folder.InTransaction));
        Assert.Equal("edited", (await Execute(folder, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].Text);
        await Assert.ThrowsAnyAsync<Exception>(() => TableEditBatch.ExecuteAsync(
            "UPDATE Book_3 SET PublishDate='discard' WHERE Id=1 RETURNING *; DELETE FROM Book_3 WHERE Id=999 RETURNING *;",
            "Book_3", folder.ExecuteAsync, () => folder.InTransaction));
        Assert.False(folder.InTransaction);
        Assert.Equal("edited", (await Execute(folder, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].Text);
        await folder.ExecuteAsync("BEGIN;");
        Assert.False(await TableEditBatch.ExecuteAsync("INSERT INTO Book_3 VALUES(104,NULL) RETURNING *;", "Book_3", folder.ExecuteAsync, () => folder.InTransaction));
        await folder.ExecuteAsync("ROLLBACK;");
        Assert.Empty((await Execute(folder, "SELECT * FROM Book_3 WHERE Id=104")).Rows);
    }

    [Fact]
    public async Task Archive_rejects_commit_after_another_writer_publishes_a_manifest()
    {
        await CreateFolder(true);
        using var first = await OpenFolder();
        using var second = await OpenFolder();
        await first.ExecuteAsync("BEGIN; UPDATE Book_3 SET PublishDate='first' WHERE Id=1;");
        await second.ExecuteAsync("UPDATE Book_3 SET PublishDate='second' WHERE Id=1;");
        await Assert.ThrowsAsync<IOException>(() => first.ExecuteAsync("COMMIT;"));
        await first.ExecuteAsync("ROLLBACK;");
        using var reopened = await OpenFolder();
        Assert.Equal("second", (await Execute(reopened, "SELECT PublishDate FROM Book_3 WHERE Id=1")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Archive_empty_table_accepts_inserts_after_all_rows_are_deleted()
    {
        await CreateFolder(true);
        using var folder = await OpenFolder();
        await folder.ExecuteAsync("DELETE FROM Book_3; INSERT INTO Book_3 VALUES(999,NULL);");
        Assert.Equal("999", (await Execute(folder, "SELECT Id FROM Book_3")).Rows.Single()[0].Text);
    }

    [Fact]
    public async Task Archive_preserves_untouched_segments_and_rekeys_deletions_across_segments()
    {
        await CreateFolder(true);
        string secondPath = Path.Combine(Data, "table-second.ndb");
        File.Copy(Segment, secondPath);
        using (var library = NewDbmsDatabase.Open(Path.Combine(Data, "library.ndb")))
        using (var tx = library.BeginTransaction())
        {
            var catalog = JsonSerializer.Deserialize<ArchiveCatalog>(tx.Get("palv/import/manifest/v1"u8.ToArray())!)!;
            catalog.Tables[0] = catalog.Tables[0] with { Segments = [new(Path.GetFileName(Segment), 0, 50), new("table-second.ndb", 50, 53)] };
            tx.Put("palv/import/manifest/v1"u8.ToArray(), JsonSerializer.SerializeToUtf8Bytes(catalog)); tx.Commit();
        }
        using (var folder = await OpenFolder())
            await folder.ExecuteAsync("UPDATE Book_3 SET PublishDate='first' WHERE Id=1; INSERT INTO Book_3 VALUES(104,'inserted');");
        using (var reader = ArchiveTableReader.TryOpen(Data)!)
        {
            Assert.Contains(reader.Manifest.Tables[0].Segments, segment => segment.File == "table-second.ndb");
            Assert.DoesNotContain(reader.Manifest.Tables[0].Segments, segment => segment.File == Path.GetFileName(Segment));
        }
        Assert.True(File.Exists(Segment));
        using (var folder = await OpenFolder())
            await folder.ExecuteAsync("DELETE FROM Book_3 WHERE Id=50 OR Id=51;");
        using var reopened = await OpenFolder();
        var rows = await reopened.ReadEditQueryAsync("SELECT Id FROM Book_3;");
        Assert.Equal(Enumerable.Range(1,104).Where(id => id != 50 && id != 51).Select(id => id.ToString(CultureInfo.InvariantCulture)), rows.Rows.Select(row => row[0].Text));
    }

    private async Task CreateFolder(bool palv, int count = RowCount)
    {
        Directory.CreateDirectory(Data);
        await File.WriteAllTextAsync(Link, "data");
        if (palv)
        {
            CreatePalv(count);
            return;
        }
        using var db = await SilverHamuSqlConnection.OpenAsync(SqlFile, create: true);
        var values = Enumerable.Range(1, count)
            .Select(id => FormattableString.Invariant($"({id},{(id % 3 == 0 ? $"'{Day}'" : "NULL")})"));
        await db.ExecuteAsync("CREATE TABLE Book_3(Id INTEGER, PublishDate TEXT); INSERT INTO Book_3 VALUES " + string.Join(",", values));
    }

    private void CreatePalv(int count)
    {
        using (var library = NewDbmsDatabase.Create(Path.Combine(Data, "library.ndb"), Guid.NewGuid()))
        using (var tx = library.BeginTransaction())
        {
            var table = new ArchiveTable("Book_3", [new("Id", "INTEGER"), new("PublishDate", "TEXT")], count,
                [new(Path.GetFileName(Segment), 0, count)]);
            tx.Put("palv/import/manifest/v1"u8.ToArray(), JsonSerializer.SerializeToUtf8Bytes(new ArchiveCatalog(1, "UTF-8", [table])));
            tx.Commit();
        }
        using var segment = NewDbmsDatabase.Create(Segment, Guid.NewGuid());
        using var transaction = segment.BeginTransaction();
        for (int id = 1; id <= count; id++)
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
