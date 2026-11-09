using Catdb.Owner.Services;
using Catdb.Sql;
using Xunit;

namespace Catdb.Owner.Sql.Tests;

public sealed class DistinctTypeTests
{
    private static SqlDatabase Open()
    {
        var store = new MemorySqlStore();
        return new(store.RequestAsync, () => store.InTransaction);
    }

    [Fact]
    public async Task Identities_are_checked_before_reading_rows()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TYPE CustomerId AS DISTINCT UUID;
            CREATE TYPE OrderId AS DISTINCT UUID;
            CREATE TABLE customers (id CustomerId PRIMARY KEY);
            CREATE TABLE orders (id OrderId PRIMARY KEY, customer CustomerId REFERENCES customers(id))
            """);
        foreach (var invalid in new[]
        {
            "SELECT * FROM orders JOIN customers ON orders.id = customers.id",
            "UPDATE orders SET customer = id",
            "INSERT INTO customers SELECT id FROM orders",
            "DELETE FROM orders WHERE id = customer",
            "SELECT id FROM orders UNION SELECT id FROM customers",
            "SELECT CAST(id AS CustomerId) FROM orders",
            "CREATE TABLE bad (id OrderId REFERENCES customers(id))",
            "SELECT CAST(NULL AS UnknownId) FROM orders"
        })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(invalid));
        var result = Assert.Single(await db.ExecuteAsync("SELECT customer FROM orders"));
        Assert.Empty(result.Rows);
        Assert.Equal("CustomerId", Assert.Single(result.ColumnTypes!));
    }

    [Fact]
    public async Task Quantities_keep_their_constraint_through_defaults_casts_and_query_copies()
    {
        var db = Open();
        await db.ExecuteAsync("""
            CREATE TYPE Quantity AS DISTINCT INTEGER CHECK (VALUE > 0);
            CREATE TABLE lines (sku TEXT, qty Quantity DEFAULT 1);
            INSERT INTO lines (sku) VALUES ('a');
            INSERT INTO lines VALUES ('b',CAST(2 AS quantity));
            CREATE TABLE copy AS SELECT qty FROM lines
            """);
        foreach (var invalid in new[]
        {
            "INSERT INTO copy VALUES (3),(0)",
            "UPDATE lines SET qty = 0",
            "SELECT CAST(0 AS Quantity)",
            "SELECT qty + qty FROM lines",
            "CREATE TABLE invalid_default (qty Quantity DEFAULT 0)"
        })
            await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync(invalid));
        var result = Assert.Single(await db.ExecuteAsync("SELECT qty FROM copy ORDER BY qty"));
        Assert.Equal(new[] { "1", "2" }, result.Rows.Select(row => row[0].Text));
        Assert.All(result.Rows, row => Assert.Equal("Quantity", row[0].TypeName));
        Assert.Equal("Quantity", Assert.Single(result.ColumnTypes!));
    }

    [Fact]
    public async Task Rolled_back_types_cannot_be_resolved_by_later_statements()
    {
        var db = Open();
        await db.ExecuteAsync("BEGIN; CREATE TYPE Price AS DISTINCT INTEGER; ROLLBACK");
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("SELECT CAST(1 AS Price)"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TABLE products (price Price)"));
        Assert.Empty(await db.GetTablesAsync());
        await db.ExecuteAsync("CREATE TYPE Price AS DISTINCT DECIMAL(6,2)");
        var type = Assert.Single(Assert.Single(await db.ExecuteAsync("SHOW TYPES")).Rows);
        Assert.Equal("Price", type[0].Text);
        Assert.Equal("DECIMAL(6,2)", type[1].Text);
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TYPE price AS DISTINCT TEXT"));
        await Assert.ThrowsAsync<SqlException>(() => db.ExecuteAsync("CREATE TYPE UUID AS DISTINCT TEXT"));
    }
}
