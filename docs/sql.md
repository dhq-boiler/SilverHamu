# SQL, Owner and compatibility

The Rust engine owns the database and its WAL. The optional .NET 10 library `Catdb.Sql` implements SQL using the same transactional key-value API. Tables, rows, indexes, views, procedures and view grants are stored in the database. The Windows Owner application and the standalone CLI share this SQL implementation. [Embedded API and CLI example](../dotnet/README.md#embedded-sql-and-cli)

## Upgrade an existing database

- Existing public catdb databases keep their configured WAL policy. The SQL layer recognizes the original NewDBMS SQL catalog. A database containing only application key-value pairs initially shows no SQL tables.
- Use an up-to-date SQL writer for schemas that use constraints, indexes or view grants. Direct writes into SQL's reserved `_catdb_owner/` namespace bypass those invariants.
- The CLI and GUI work with the database file directly. Close the current owning process before opening the same file in another one. For simultaneous remote clients, run the service.
- SQL features in this checkout require building the matching native library and managed project. Published client package versions and the default container image have their own capabilities.

## Server-side SQL

Build the server with `cargo build --release --locked --features service --bin catdb-server` and build the standalone SQL host. Add `--sql-host <absolute-path-to-Catdb.Sql.Host>` to the server's normal startup command. On Windows use the host's `.exe`. Keep its build output together. The server owns the transaction; the configured host interprets the SQL and requests the necessary operations over private pipes.

Keep existing TLS database grants when upgrading. A client must check the authenticated server capabilities before submitting SQL. Raw key-value access, direct file ownership and a grant to read one SQL view have different permission boundaries. [Service accounts](accounts.md)

## SQL coverage

| Operation | Supported scope |
|---|---|
| Data definition and updates | CREATE TABLE AS SELECT, INSERT from VALUES or a query, expression assignments, DELETE and MERGE. ALTER TABLE supports adding, renaming and dropping columns, changing a compatible type, and setting or dropping a default or NOT NULL. |
| Constraints | NOT NULL, UNIQUE, CHECK, single or composite primary and foreign keys, ON UPDATE CASCADE and ON DELETE CASCADE. |
| Indexes | Single-column and composite indexes, leading equality and range predicates, REINDEX for rebuilding older indexes |
| Queries | DISTINCT, CASE, CAST, arithmetic, BETWEEN, IN, NOT, inner/outer/cross/natural joins and USING, GROUP BY, HAVING, aggregates, ORDER BY and OFFSET/FETCH. |
| Combining queries | Scalar and correlated subqueries, EXISTS, derived tables, UNION [ALL], INTERSECT, EXCEPT, WITH and WITH RECURSIVE. |
| Views and procedures | Base-table views and parameterless procedures. A single-table view can accept writes when it projects distinct base columns directly and its definition has no grouping, DISTINCT, windows or pagination. |
| Access control | GRANT and REVOKE SELECT on views and INSERT, UPDATE or DELETE on named tables or views. Write privileges are independent of each other and of SELECT. |
| Transactions | BEGIN or START TRANSACTION, COMMIT, ROLLBACK, named SAVEPOINT, ROLLBACK TO [SAVEPOINT] and RELEASE [SAVEPOINT]. |

SQL tables have at most 64 columns and 4,096 rows. Each join step, materialized subquery, CTE and set operand has a 4,096-row limit. An oversized intermediate result raises an error, including when reading an archive. A top-level SELECT without an explicit fetch count returns up to 4,096 rows. The native key-value store has its own resource limits. Physical power-loss qualification remains a separate hardware test.

### SQL types

A column declaration determines how SQL validates, compares and returns its values. Choose a named type when two columns share a representation but exchanging their values would be an error.

```sql
CREATE TYPE CustomerId AS DISTINCT UUID;
CREATE TYPE OrderId AS DISTINCT UUID;
CREATE TYPE JpyAmount AS DISTINCT DECIMAL(18,0);
CREATE TYPE Quantity AS DISTINCT INTEGER CHECK (VALUE > 0);

CREATE TABLE customers (id CustomerId PRIMARY KEY);
CREATE TABLE orders (
    id OrderId PRIMARY KEY,
    customer CustomerId NOT NULL REFERENCES customers(id),
    paid JpyAmount NOT NULL,
    qty Quantity NOT NULL
);
INSERT INTO customers VALUES ('558c5a77-75ae-41cf-abb4-e12001e8b1a4');
INSERT INTO orders VALUES ('34e83e3e-f136-4944-9210-fe4d1deca1b2',
    '558c5a77-75ae-41cf-abb4-e12001e8b1a4', 1200, 2);
SELECT orders.paid FROM orders JOIN customers ON orders.customer = customers.id;
SHOW TYPES;
```

Joining on `orders.id = customers.id` raises a type error, even with empty tables. A literal assigned to a column is checked against its declaration. In a predicate, construct a typed value explicitly, for example `WHERE qty = CAST(2 AS Quantity)`. A type's `CHECK` evaluates its base value as `VALUE` and rejects a false result. Use `NOT NULL` on the column when a value is required.

Same-type assignments, comparisons and foreign keys preserve the type's identity. Query results report declared names in `ColumnTypes`, including for empty results. Each non-null named value also carries `TypeName` alongside its primitive `Kind` and `Text`.

To add two amounts and keep their currency, enable addition for their type once in the database. This works for existing columns as well as new ones.

```sql
ALTER TYPE JpyAmount ENABLE OPERATOR +;
CREATE TABLE invoice_lines (subtotal JpyAmount, tax JpyAmount);
INSERT INTO invoice_lines VALUES (1000, 100);
SELECT subtotal + tax AS total FROM invoice_lines;
-- total = 1100, column type = JpyAmount
```

Addition can be enabled for a distinct type based on `INTEGER`, `DECIMAL(p,s)` or `REAL`. Both operands and the result have that distinct type. Each addition uses the base numeric addition and validates the result against the declared range, scale and `CHECK`. An inexact or out-of-range result raises an error. A NULL operand yields NULL with the declared result column type. A different currency or a plain numeric operand is a type error. A literal can be given a currency explicitly, for example `subtotal + CAST(100 AS JpyAmount)`. The lexer also accepts fullwidth `＋` in expressions. Other arithmetic and aggregates retain their existing rules, for example `SUM(CAST(paid AS DECIMAL(18,0)))` sums the base numbers.

| Base type | Input and guarantees |
|---|---|
| `INTEGER`, `REAL`, `BOOLEAN`, `TEXT`, `BLOB` | Signed 64-bit integer, legacy decimal-backed number, true/false, text and bytes. Text and binary columns can declare a maximum length. |
| `DECIMAL(p,s)` | Exact decimal with 1–28 digits of precision and 0–p fractional digits. Excess nonzero fractional digits and overflow raise an error. Trailing zeroes may be discarded on input. Numeric arithmetic and sums use the engine's decimal evaluator. |
| `UUID` | Hyphenated hexadecimal UUID, returned in lowercase. |
| `DATE`, `TIME` | Calendar date such as `2024-02-29`, or local clock time such as `09:30:00`. |
| `LOCAL_DATETIME` | Local date and time, for example `2026-11-10T09:30:00`. |
| `INSTANT` | Date and time with an explicit offset or `Z`. Equivalent instants compare equal and are returned in UTC. |
| `DURATION` | Signed elapsed time such as `-1.02:30:00` (one day, two hours and thirty minutes before zero). Time, timestamp and duration inputs support up to seven fractional second digits. |

Type names are case-insensitive and results use the declared spelling. Up to 256 distinct types can be created per database. Definitions participate in transactions and can use a built-in base with an optional length, precision or scale. Use `CREATE TYPE [IF NOT EXISTS]` to declare a type, `ALTER TYPE ... ENABLE OPERATOR +` to enable addition and `SHOW TYPES` to list types. A type check may refer to `VALUE` and cast to built-in types. When adopting a named type on an existing column, `ALTER TABLE ... ALTER COLUMN ... TYPE ...` validates the existing values.

Existing stored rows and schemas retain their original interpretation, including bare `DECIMAL`, `FLOAT` and `DOUBLE` as aliases for `REAL`. New numeric literals that would silently lose significant digits in the decimal parser raise an error. Use matching SQL engines for all writers to a database that uses named types.

### Window functions

Use `OVER (PARTITION BY ... ORDER BY ...)` with `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `NTILE`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`, `COUNT`, `SUM`, `AVG`, `MIN` or `MAX`. Both clauses are optional and accept column names. `LAG` and `LEAD` accept a nonnegative integer offset and an optional literal default. `NTILE` takes a positive integer bucket count.

With ORDER BY, the default frame runs from the partition start through the current row and its peers. Without ORDER BY, it covers the whole partition. An explicit `ROWS BETWEEN ... AND ...` frame accepts current-row, preceding and following bounds. This query shows each reading next to its previous and cumulative values.

```sql
SELECT sensor, sample_no, reading,
       LAG(reading) OVER (PARTITION BY sensor ORDER BY sample_no) AS previous,
       SUM(reading) OVER (PARTITION BY sensor ORDER BY sample_no
                          ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS total
FROM readings
ORDER BY sensor, sample_no;
```

### Role writes

For a view-only application on a token-based service, configure `--sql-host`, `--read-role app_viewer` and `--read-token-file <private-file>`, with role writes disabled (the default). Give the application only this role token. An administrator publishes a view with `GRANT SELECT ON public_books TO app_viewer`. The application can query its granted views and list them with `SHOW VIEWS`. Direct key reads, range scans, base-table queries and writes are denied. Once `REVOKE SELECT ON public_books FROM app_viewer` commits, the next query on an already authenticated connection is denied too. An account-mode database `reader` grant permits raw key reads and is unsuitable for this role. Use loopback or an encrypted tunnel for token-mode connections.

An administrator can run `GRANT UPDATE ON public_books TO editor` and later `REVOKE UPDATE ON public_books FROM editor`. An embedded host supplies the authenticated role to `ExecuteRoleAsync(sql, role)`. On the token-based SQL service, configure the role and enable `--allow-role-writes` to use these grants. The read-only APIs continue to accept only view queries. For an updatable filtered view, UPDATE and DELETE select only visible rows. INSERT uses the exposed columns and the underlying table's defaults and constraints.
