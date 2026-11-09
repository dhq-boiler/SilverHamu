using System.Text.Json.Nodes;
using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class SqlTests
{
    private static (SqlDatabase Db, MemorySqlStore Store) Database()
    {
        var store = new MemorySqlStore();
        return (new(store.RequestAsync, () => store.InTransaction), store);
    }

    [Fact]
    public async Task Primary_key_lookup_uses_point_reads_and_legacy_tables_can_be_reindexed()
    {
        var store = new MemorySqlStore();
        var operations = new List<string>();
        var db = new SqlDatabase(request =>
        {
            operations.Add(request["op"]!.ToString());
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await db.ExecuteAsync("CREATE TABLE books (id REAL PRIMARY KEY, title TEXT); INSERT INTO books VALUES (1.0,'one'),(2,'two')");
        operations.Clear();
        var hit = (await db.ExecuteAsync("SELECT title FROM books WHERE 1.00 = ID"))[0];
        Assert.Equal("one", Assert.Single(Assert.Single(hit.Rows)).Display);
        Assert.Equal(new[] { "begin", "get", "get", "get", "rollback" }, operations);
        operations.Clear();
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM books WHERE id=3"))[0].Rows);
        Assert.DoesNotContain("scan", operations);
        await db.ExecuteAsync("BEGIN; UPDATE books SET id=3 WHERE id=1");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM books WHERE id=1"))[0].Rows);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM books WHERE id=3"))[0].Rows);
        await db.ExecuteAsync("ROLLBACK; DELETE FROM books WHERE id=2");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM books WHERE id=2"))[0].Rows);

        // Reproduce the on-disk schema of an older writer, including an unindexed insert.
        var legacy = new SqlDatabase(request =>
        {
            if (request["op"]!.ToString() == "put" && request["key"]!.ToString().StartsWith(SqlNames.Hex("_catdb_owner/sql/v1/table/"), StringComparison.Ordinal))
            {
                var schema = JsonNode.Parse(Convert.FromHexString(request["value"]!.ToString()))!.AsObject();
                schema.Remove("PrimaryIndex");
                request["value"] = Convert.ToHexString(System.Text.Json.JsonSerializer.SerializeToUtf8Bytes(schema));
            }
            return store.RequestAsync(request);
        }, () => store.InTransaction);
        await legacy.ExecuteAsync("CREATE TABLE old (id INT PRIMARY KEY); INSERT INTO old VALUES (42)");
        operations.Clear();
        Assert.Single((await db.ExecuteAsync("SELECT * FROM old WHERE id=42"))[0].Rows);
        Assert.Contains("scan", operations);
        await db.ExecuteAsync("REINDEX old");
        operations.Clear();
        Assert.Single((await db.ExecuteAsync("SELECT * FROM old WHERE id=42"))[0].Rows);
        Assert.DoesNotContain("scan", operations);
        await db.ExecuteAsync("DROP TABLE old; CREATE TABLE old (id INT PRIMARY KEY)");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM old WHERE id=42"))[0].Rows);
    }

    [Fact]
    public async Task EQ_CRUD_preserves_types_defaults_and_named_projections()
    {
        var (db, store) = Database();
        await db.ExecuteAsync("CREATE TABLE books (id INT PRIMARY KEY, title TEXT NOT NULL, price REAL, active BOOLEAN DEFAULT TRUE, cover BLOB); INSERT INTO books (id,title,price,cover) VALUES (1,'猫の本',12.5,X'00FF'),(2,'Another',8,NULL);");
        var result = (await db.ExecuteAsync("SELECT title AS [書名], price, active, cover FROM books WHERE id = 1;"))[0];
        Assert.Equal(new[] { "書名", "price", "active", "cover" }, result.Columns);
        Assert.Equal(new[] { "猫の本", "12.5", "true", "0x00FF" }, Assert.Single(result.Rows).Select(x => x.Display));
        Assert.False(store.InTransaction);
        await db.ExecuteAsync("UPDATE books SET price = 20 WHERE id = 1; DELETE FROM books WHERE id = 2;");
        Assert.Equal("20", (await db.ExecuteAsync("SELECT price FROM books;"))[0].Rows[0][0].Display);
        Assert.Single(await db.GetTablesAsync());
        await db.ExecuteAsync("DROP TABLE books;");
        Assert.Empty(await db.GetTablesAsync());
    }

    [Fact]
    public async Task EQ_quoted_names_strings_comments_and_case_are_preserved()
    {
        var (db, _) = Database();
        await db.ExecuteAsync("-- ; ignored\n CREATE TABLE [本の一覧] ([識別子] INTEGER PRIMARY KEY, [書名] TEXT); /* ; */ INSERT INTO [本の一覧] VALUES (1, N'O''Brien; -- /* */');");
        Assert.Equal("O'Brien; -- /* */", (await db.ExecuteAsync("SELECT [書名] FROM [本の一覧];"))[0].Rows[0][0].Display);
        await db.ExecuteAsync("CREATE TABLE Mixed (Id INTEGER); INSERT INTO MIXED VALUES (2);");
        Assert.Equal("2", (await db.ExecuteAsync("select ID from mixed;"))[0].Rows[0][0].Display);
    }

    [Fact]
    public async Task EQ_conditions_sorting_count_and_null_follow_SQL_semantics()
    {
        var (db, _) = Database();
        await db.ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t VALUES (1,'cat'),(2,NULL),(10,'car');");
        var result = (await db.ExecuteAsync("SELECT id FROM t WHERE name LIKE 'ca%' OR id = 2 AND name IS NOT NULL ORDER BY id DESC LIMIT 1 OFFSET 0;"))[0];
        Assert.Equal("10", Assert.Single(result.Rows)[0].Display);
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM t WHERE name = NULL;"))[0].Rows);
        Assert.Equal("1", (await db.ExecuteAsync("SELECT COUNT(*) FROM t WHERE name IS NULL;"))[0].Rows[0][0].Display);
        Assert.Equal("0", (await db.ExecuteAsync("SELECT COUNT(*) FROM t WHERE id < 0;"))[0].Rows[0][0].Display);
        Assert.Equal("3", (await db.ExecuteAsync("SELECT COUNT(*) FROM t;"))[0].Rows[0][0].Display);
        Assert.Equal("1", (await db.ExecuteAsync("SELECT 1 AS ok;"))[0].Rows[0][0].Display);
    }

    [Theory]
    [InlineData("INSERT INTO t VALUES (1,'duplicate');")]
    [InlineData("INSERT INTO t VALUES (NULL,'missing');")]
    [InlineData("INSERT INTO t VALUES (2,NULL);")]
    [InlineData("INSERT INTO t VALUES ('text','wrong');")]
    [InlineData("INSERT INTO t VALUES (2,'too long');")]
    [InlineData("INSERT INTO t VALUES (2,'abc'),(1,'dup');")]
    [InlineData("UPDATE t SET id = 1.5;")]
    [InlineData("UPDATE t SET missing = 1;")]
    [InlineData("UPDATE t SET name = NULL;")]
    public async Task EQ_constraints_reject_the_entire_statement_without_changes(string sql)
    {
        var (db, store) = Database();
        await db.ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT(3) NOT NULL); INSERT INTO t VALUES (1,'abc');");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(sql));
        var rows = (await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows;
        Assert.Equal(new[] { "1", "abc" }, Assert.Single(rows).Select(x => x.Display));
        Assert.False(store.InTransaction);
    }

    [Theory]
    [InlineData("SELECT * FROM t LIMIT -1;")]
    [InlineData("SELECT * FROM t LIMIT 4097;")]
    [InlineData("SELECT * FROM t LIMIT 1.5;")]
    [InlineData("SELECT TOP 1 * FROM t LIMIT 1;")]
    [InlineData("SELECT * FROM t WHERE id IN ();")]
    [InlineData("SELECT * FROM t CROSS JOIN other ON t.id = other.id;")]
    [InlineData("CREATE TABLE a (id UNKNOWN);")]
    [InlineData("CREATE TABLE a (id INT, ID TEXT);")]
    [InlineData("SELECT 'unclosed;")]
    [InlineData("/* unclosed")]
    [InlineData("INSERT INTO t VALUES (1,'x'); INVALID;")]
    public void ER_invalid_or_unsupported_scripts_are_rejected_before_execution(string sql) => Assert.Throws<SqlException>(() => SqlParser.Parse(sql));

    [Fact]
    public async Task BV_zero_top_and_maximum_rows_are_exact()
    {
        var (db, _) = Database();
        await db.ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY);");
        await db.ExecuteAsync("INSERT INTO t VALUES " + string.Join(',', Enumerable.Range(0, 4096).Select(i => $"({i})")) + ";");
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM t LIMIT 0;"))[0].Rows);
        Assert.Single((await db.ExecuteAsync("SELECT TOP(1) * FROM t;"))[0].Rows);
        Assert.Equal(4096, (await db.ExecuteAsync("SELECT * FROM t LIMIT 4096;"))[0].Rows.Count);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES (4096);"));
    }

    [Fact]
    public async Task ST_explicit_transactions_commit_or_rollback_schema_and_rows()
    {
        var (db, store) = Database();
        await db.ExecuteAsync("BEGIN; CREATE TABLE t (id INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);");
        Assert.True(store.InTransaction);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
        await db.ExecuteAsync("ROLLBACK;");
        Assert.Empty(await db.GetTablesAsync());
        await db.ExecuteAsync("BEGIN; CREATE TABLE t (id INTEGER PRIMARY KEY); INSERT INTO t VALUES (1); COMMIT;");
        await db.ExecuteAsync("BEGIN; DELETE FROM t; ROLLBACK;");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
        Assert.False(store.InTransaction);
    }

    [Fact]
    public async Task DT_validation_error_keeps_explicit_transaction_usable()
    {
        var (db, store) = Database();
        await db.ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY); BEGIN; INSERT INTO t VALUES (1);");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES (1);"));
        Assert.True(store.InTransaction);
        await db.ExecuteAsync("COMMIT;");
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task DT_mid_write_failure_rolls_back_the_transaction(bool explicitTransaction)
    {
        var (setup, store) = Database();
        await setup.ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY);");
        if (explicitTransaction) await setup.ExecuteAsync("BEGIN; INSERT INTO t VALUES (9);");
        var writes = 0;
        var db = new SqlDatabase(request => request["op"]!.ToString() == "put" && ++writes == 2 ? throw new IOException("injected storage failure") : store.RequestAsync(request), () => store.InTransaction);
        await Assert.ThrowsAnyAsync<Exception>(() => db.ExecuteAsync("INSERT INTO t VALUES (1),(2);"));
        Assert.False(store.InTransaction);
        Assert.Empty((await setup.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task ER_unknown_columns_on_empty_tables_and_invalid_tail_do_not_succeed()
    {
        var (db, _) = Database();
        await db.ExecuteAsync("CREATE TABLE t (id INTEGER);");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT missing FROM t;"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT * FROM t ORDER BY missing;"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES (1); nonsense;"));
        Assert.Empty((await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task ER_raw_key_value_data_is_preserved_and_not_shown_as_tables()
    {
        var (db, store) = Database();
        await store.RequestAsync(new JsonObject { ["op"] = "begin" });
        await store.RequestAsync(new JsonObject { ["op"] = "put", ["key"] = "626F6F6B2F31", ["value"] = "00FF" });
        await store.RequestAsync(new JsonObject { ["op"] = "commit" });
        await db.ExecuteAsync("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1); DROP TABLE t;");
        Assert.Empty(await db.GetTablesAsync());
        await store.RequestAsync(new JsonObject { ["op"] = "begin" });
        Assert.Equal("00FF", (await store.RequestAsync(new JsonObject { ["op"] = "get", ["key"] = "626F6F6B2F31" }))["value"]!.ToString());
    }

    [Fact]
    public async Task ER_lost_commit_response_is_never_replayed()
    {
        var (setup, store) = Database();
        await setup.ExecuteAsync("CREATE TABLE t (id INTEGER);");
        var commits = 0;
        var db = new SqlDatabase(async request =>
        {
            var response = await store.RequestAsync(request);
            if (request["op"]!.ToString() == "commit") { commits++; throw new IOException("commit outcome unknown"); }
            return response;
        }, () => store.InTransaction);
        await Assert.ThrowsAsync<IOException>(() => db.ExecuteAsync("INSERT INTO t VALUES (1);"));
        Assert.Equal(1, commits);
        Assert.Single((await setup.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Theory]
    [InlineData("-9223372036854775808", true)]
    [InlineData("9223372036854775807", true)]
    [InlineData("-9223372036854775809", false)]
    [InlineData("9223372036854775808", false)]
    public async Task BV_integer_range_is_signed_64_bit(string number, bool valid)
    {
        var (db, _) = Database(); await db.ExecuteAsync("CREATE TABLE t (x INTEGER);");
        if (valid) { await db.ExecuteAsync($"INSERT INTO t VALUES ({number});"); Assert.Equal(number, (await db.ExecuteAsync("SELECT x FROM t;"))[0].Rows[0][0].Display); }
        else await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync($"INSERT INTO t VALUES ({number});"));
    }

    [Fact]
    public async Task BV_row_size_limit_rejects_oversized_data_without_partial_inserts()
    {
        var (db, _) = Database(); await db.ExecuteAsync("CREATE TABLE t (value TEXT);");
        await db.ExecuteAsync("INSERT INTO t VALUES ('" + new string('a', 200000) + "');");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES ('small'),('" + new string('a', 262144) + "');"));
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task ST_forbidden_transaction_transitions_leave_existing_work_intact()
    {
        var (db, store) = Database();
        await Assert.ThrowsAnyAsync<Exception>(() => db.ExecuteAsync("COMMIT;"));
        await db.ExecuteAsync("BEGIN; CREATE TABLE t (id INTEGER);");
        await Assert.ThrowsAnyAsync<Exception>(() => db.ExecuteAsync("BEGIN;"));
        Assert.True(store.InTransaction);
        await db.ExecuteAsync("COMMIT;"); Assert.Single(await db.GetTablesAsync());
    }

    [Fact]
    public async Task ER_runtime_batch_failure_reports_prior_commits()
    {
        var (db, _) = Database();
        var error = await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE t (id INTEGER PRIMARY KEY); INSERT INTO t VALUES (1); INSERT INTO t VALUES (1);"));
        Assert.Contains("先行2文", error.Message);
        Assert.Single((await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows);
    }

    [Fact]
    public async Task EQ_uniqueness_compares_numeric_value_and_allows_multiple_nulls()
    {
        var (db, _) = Database();
        await db.ExecuteAsync("CREATE TABLE t (value REAL UNIQUE); INSERT INTO t VALUES (1), (NULL), (NULL);");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES (1.00);"));
        Assert.Equal(3, (await db.ExecuteAsync("SELECT * FROM t;"))[0].Rows.Count);
    }

    [Fact]
    public async Task BV_result_column_limit_includes_expanded_stars()
    {
        var (db, _) = Database();
        Assert.Equal(64, (await db.ExecuteAsync("SELECT " + string.Join(',', Enumerable.Repeat("1", 64))))[0].Columns.Count);
        Assert.Throws<SqlException>(() => SqlParser.Parse("SELECT " + string.Join(',', Enumerable.Repeat("1", 65))));
        await db.ExecuteAsync("CREATE TABLE t (x INTEGER);");
        Assert.Equal(64, (await db.ExecuteAsync("SELECT " + string.Join(',', Enumerable.Repeat("*", 64)) + " FROM t"))[0].Columns.Count);
        await db.ExecuteAsync("CREATE TABLE two_columns (x INTEGER, y INTEGER);");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT " + string.Join(',', Enumerable.Repeat("*", 33)) + " FROM two_columns"));
    }
}
