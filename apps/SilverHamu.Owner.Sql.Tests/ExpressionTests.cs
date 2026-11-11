using SilverHamu.Owner.Services;
using SilverHamu.Sql;
using Xunit;

namespace SilverHamu.Owner.Sql.Tests;

public sealed class ExpressionTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Numeric_precedence_cast_and_both_case_forms_work_in_projections()
    {
        var db = Open();
        var row = Assert.Single((await db.ExecuteAsync("""
            SELECT 2+3*4, -(2+3)*4, CAST('3.75' AS INTEGER),
                   CASE 2 WHEN 1 THEN 'one' WHEN 2 THEN 'two' END,
                   CASE WHEN NULL THEN 1/0 WHEN 2 BETWEEN 1 AND 3 THEN CAST('7' AS INT) ELSE 1/0 END
            """))[0].Rows);
        Assert.Equal(new[] { "14", "-20", "3", "two", "7" }, row.Select(v => v.Display));
    }

    [Fact]
    public async Task Between_membership_and_negation_preserve_unknown()
    {
        var row = Assert.Single((await Open().ExecuteAsync("""
            SELECT 2 NOT IN (1,NULL), 2 IN (2.00,NULL), NULL IN (1), NOT NULL,
                   9 BETWEEN NULL AND 5, 3 BETWEEN NULL AND 5,
                   NOT 2 BETWEEN 1 AND 3 OR 1 IN (2,3), 4 NOT BETWEEN 1 AND 3
            """))[0].Rows);
        Assert.Equal(new[] { "NULL", "true", "NULL", "NULL", "false", "NULL", "false", "true" }, row.Select(v => v.Display));
    }

    [Fact]
    public async Task Having_filters_groups_before_pagination_and_window_functions()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE sales(dept TEXT, v REAL);
            INSERT INTO sales VALUES ('a',1),('a',1.0),('a',NULL),('b',5),('b',3),('c',NULL);
            """);
        var result = (await db.ExecuteAsync("""
            SELECT dept, COUNT(v), COUNT(DISTINCT v) AS unique_v, MIN(v), MAX(v),
                   SUM(COUNT(v)) OVER(ORDER BY dept) AS running_count
            FROM sales GROUP BY dept HAVING COUNT(v) > 0 AND (MAX(v) - MIN(v)) BETWEEN 0 AND 2
            ORDER BY dept
            """))[0];
        Assert.Equal(2, result.Rows.Count);
        Assert.Equal("a", result.Rows[0][0].Display);
        Assert.Equal(new decimal[] { 2, 1, 1, 1, 2 }, result.Rows[0].Skip(1)
            .Select(v => decimal.Parse(v.Display, System.Globalization.CultureInfo.InvariantCulture)));
        Assert.Equal(new[] { "b", "2", "2", "3", "5", "4" }, result.Rows[1].Select(v => v.Display));
        var empty = Assert.Single((await db.ExecuteAsync("""
            SELECT COUNT(v), COUNT(DISTINCT v), MIN(dept), MAX(dept) FROM sales WHERE v > 9 HAVING COUNT(*)=0
            """))[0].Rows);
        Assert.Equal(new[] { "0", "0", "NULL", "NULL" }, empty.Select(v => v.Display));
    }

    [Fact]
    public async Task Checks_and_group_validation_cover_nested_expressions_on_empty_inputs()
    {
        var db = Open();
        await db.ExecuteAsync("CREATE TABLE t(x INT CHECK (NOT (x*2 NOT BETWEEN 0 AND 10)), label TEXT)");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("INSERT INTO t VALUES (6,'too big')"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT x+1, COUNT(*) FROM t"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT COUNT(*) FROM t HAVING x>0"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT COUNT(*) FROM t WHERE MAX(x)=1"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT SUM(label) FROM t"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT CAST('x' AS INTEGER)"));
    }

    [Fact]
    public async Task View_description_and_read_only_scans_validate_without_evaluating_an_empty_group()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TABLE t(x INT); INSERT INTO t VALUES (1),(2);
            CREATE VIEW ratios AS SELECT 10/COUNT(*) AS ratio FROM t;
            """);
        Assert.Equal("5", (await db.ExecuteAsync("SELECT ratio FROM ratios"))[0].Rows.Single()[0].Display);
        var query = Assert.IsType<SelectRows>(SqlParser.Parse("SELECT 10/COUNT(*) FROM t HAVING COUNT(*)>1").Single());
        var schema = Assert.Single(await db.GetTablesAsync());
        static async IAsyncEnumerable<SqlRow> Scan(string name)
        {
            await Task.CompletedTask;
            yield return new("1", new() { ["x"] = SqlValue.Number("1") });
            yield return new("2", new() { ["x"] = SqlValue.Number("2") });
        }
        var readOnly = await SqlDatabase.QueryReadOnlyAsync(query, _ => schema, Scan);
        Assert.Equal("5", Assert.Single(readOnly.Rows)[0].Display);
    }
}
