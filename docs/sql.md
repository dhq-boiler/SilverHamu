# SQL, Owner and compatibility

The Rust engine owns the database and its WAL. The optional .NET 10 library `SilverHamu.Sql` implements SQL using the same transactional key-value API. Tables, rows, indexes, views, procedures and view grants are stored in the database. The Windows Owner application and the standalone CLI share this SQL implementation. [Embedded API and CLI example](../dotnet/README.md#embedded-sql-and-cli)

## Upgrade an existing database

- Existing public silverhamu databases keep their configured WAL policy. The SQL layer recognizes the original NewDBMS SQL catalog. A database containing only application key-value pairs initially shows no SQL tables.
- Use an up-to-date SQL writer for schemas that use constraints, indexes or view grants. Direct writes into SQL's reserved `_catdb_owner/` namespace bypass those invariants.
- The CLI and GUI work with the database file directly. Close the current owning process before opening the same file in another one. For simultaneous remote clients, run the service.
- SQL features in this checkout require building the matching native library and managed project. Published client package versions and the default container image have their own capabilities.

## Server-side SQL

Build the server with `cargo build --release --locked --features service --bin silverhamu-server` and build the standalone SQL host. Add `--sql-host <absolute-path-to-SilverHamu.Sql.Host>` to the server's normal startup command. On Windows use the host's `.exe`. Keep its build output together. The server owns the transaction; the configured host interprets the SQL and requests the necessary operations over private pipes.

Keep existing TLS database grants when upgrading. A client must check the authenticated server capabilities before submitting SQL. Raw key-value access, direct file ownership and a grant to read one SQL view have different permission boundaries. [Service accounts](accounts.md)

## SQL coverage

| Operation | Supported scope |
|---|---|
| Data definition and updates | CREATE TABLE AS SELECT, INSERT, UPDATE, DELETE, MERGE and transactional TRUNCATE TABLE. INSERT supports ON CONFLICT; INSERT, UPDATE and DELETE support RETURNING. |
| Constraints | NOT NULL, UNIQUE, CHECK, single or composite primary and foreign keys, ON UPDATE and ON DELETE actions including CASCADE, SET NULL and SET DEFAULT. |
| Indexes | Single-column and composite indexes, unique and partial indexes, leading equality and range predicates, REINDEX for rebuilding older indexes. |
| Queries | DISTINCT, CASE, CAST, arithmetic, BETWEEN, IN, NOT, inner/outer/cross/natural joins and USING, GROUP BY, HAVING, aggregates, ORDER BY and OFFSET/FETCH. |
| Combining queries | Scalar and correlated subqueries, EXISTS, derived tables, UNION [ALL], INTERSECT, EXCEPT, WITH and WITH RECURSIVE. |
| Views and procedures | Base-table views and parameterless procedures. A single-table view can accept writes when it projects distinct base columns directly and its definition has no grouping, DISTINCT, windows or pagination. |
| Access control | GRANT and REVOKE SELECT on views and INSERT, UPDATE or DELETE on named tables or views. Write privileges are independent of each other and of SELECT. |
| Transactions | BEGIN or START TRANSACTION, COMMIT, ROLLBACK, named SAVEPOINT, ROLLBACK TO [SAVEPOINT] and RELEASE [SAVEPOINT]. |

SQL tables have at most 64 columns and 4,096 rows. Each join step, materialized subquery, CTE and set operand has a 4,096-row limit. An oversized intermediate result raises an error, including when reading an archive. A top-level SELECT without an explicit fetch count returns up to 4,096 rows. The native key-value store has its own resource limits. Physical power-loss qualification remains a separate hardware test.

### Writes and concurrency

Use a primary key or an unconditional unique key as the conflict target when accumulating quantities. In this example, both the insert and update paths return the final quantity:

```sql
CREATE TABLE stock (sku TEXT PRIMARY KEY, qty INT);
INSERT INTO stock VALUES ('A', 3)
ON CONFLICT (sku) DO UPDATE SET qty = stock.qty + excluded.qty
RETURNING sku, qty;
```

`CREATE UNIQUE INDEX active_sku ON listings(sku) WHERE active = TRUE` enforces uniqueness among qualifying rows with non-NULL key components. A partial index can serve a query when its conditions establish the index predicate. For a foreign key with `SET DEFAULT`, the replacement must still reference an existing parent unless a NULL component exempts it. A failed referential action cancels the statement.

Use a locking read when an application needs to inspect a row and then update it in the same transaction:

```sql
BEGIN;
SELECT qty FROM stock WHERE sku = 'A' FOR UPDATE;
UPDATE stock SET qty = qty - 1 WHERE sku = 'A';
COMMIT;
```

`FOR UPDATE` locks the selected base rows after filtering, ordering and paging. It accepts a single base table without DISTINCT, grouping, windows, joins or subqueries. Other transactions can read their snapshots; a competing reservation or write conflicts immediately, and commit also checks writes staged before the reservation. Commit, rollback and disconnect release locks. Rolling back to a savepoint releases locks acquired after it. At most 4,096 keys can be reserved per transaction. An autocommit query releases its locks at statement end. Use matching native and service binaries; archive folders and view-only tokens cannot acquire locks.

`CREATE SEQUENCE ticket_numbers START WITH 100 INCREMENT BY 1` creates a transactional sequence. Allocate a value with `SELECT NEXT VALUE FOR ticket_numbers` or directly in `INSERT ... VALUES`. Rolling back also rolls back the allocation.

Administrators can define row-level AFTER triggers. In an INSERT trigger `NEW` holds the inserted row; in a DELETE trigger `OLD` holds the removed row. UPDATE supplies both. Trigger actions run in the transaction that made the change, including when a granted writer caused the change. Limit the actions an administrator attaches to writable tables accordingly.

```sql
CREATE TABLE stock_audit (sku TEXT, previous_qty INT, new_qty INT);
CREATE TRIGGER stock_changed AFTER UPDATE ON stock FOR EACH ROW
WHEN OLD.qty <> NEW.qty
BEGIN
    INSERT INTO stock_audit VALUES (NEW.sku, OLD.qty, NEW.qty);
END;
```

`TRUNCATE TABLE stock` clears rows and indexes while preserving the schema. It rejects references from other tables and does not fire DELETE triggers. Use DELETE when row-level trigger actions are required.

### Query plans and statistics

Use `EXPLAIN` to see how a SELECT will access data. For example, these statements collect statistics on two existing tables and inspect the plan for matching order lines to prices by product and currency:

```sql
ANALYZE order_lines;
ANALYZE prices;
EXPLAIN SELECT l.product_id, p.price
FROM order_lines AS l
JOIN prices AS p ON l.product_id = p.product_id AND l.currency = p.currency;
```

Each plan row gives a step, operation, relation and detail. Where available, `estimated_rows` describes the access or join output and `estimated_cost` gives relative work units. A table scan can be cheaper than fetching many rows through an index. A primary-key lookup needs at most one row, and an ordered secondary index can narrow a scan using equality on leading columns followed by a range. The full predicate is still checked on the candidates.

`ANALYZE table_name` scans a base table and stores its row count, non-NULL counts, distinct counts and frequencies for up to eight repeated values per column. The statistics follow transaction commit and rollback. Run it again after large data changes to refresh the estimates. Tables without statistics use a default cardinality of 256. Estimates guide plan selection and never limit how many matching rows a statement returns.

For inner joins with explicit equality keys, the planner compares connected join orders and the cost of nested loops versus building a hash table. Outer joins, `USING`, natural joins, cross joins and joins with additional filters preserve the written order. Joins materialize their inputs; a join key alone does not trigger an indexed lookup on the other table. If stale estimates lead to an oversized reordered intermediate, the executor can retry in written order under the same snapshot. The 4,096-row intermediate limit still applies.

An explicit `ON` accepts a scalar boolean predicate. Equalities between inputs can form a composite hash key; a join such as `ON l.amount BETWEEN p.minimum AND p.maximum` compares candidate pairs directly. The full predicate is applied before deciding which rows of an outer join need NULL padding. Aggregates and subqueries inside `ON` are unsupported.

`EXPLAIN` inspects metadata without executing the SELECT against stored rows. To measure execution, use `EXPLAIN ANALYZE` on a SELECT or WITH query. It executes the query once and returns the measurements in place of the query results. Execution errors, such as division by zero, propagate normally. Both forms are available on full SQL connections and in Owner's folder mode. View-only service tokens cannot inspect plans.

In a folder session, `ANALYZE Book_3` collects statistics in memory. Closing or reopening the folder clears them. For a large segmented archive the row count comes from the manifest and the column estimates use sampled rows. To inspect a concrete read, run `EXPLAIN SELECT * FROM Book_3 WHERE PublishDate IS NOT NULL FETCH FIRST 10 ROWS ONLY` and then prefix the same query with `EXPLAIN ANALYZE` to measure it. A simple limit can stop the scan early; an unfiltered `COUNT(*)` on a segmented archive can use the manifest count without reading table rows.

```sql
EXPLAIN ANALYZE SELECT product_id FROM prices WHERE price > 100 ORDER BY product_id;
```

| Profile column | How to read it |
|---|---|
| `estimated_rows` | Planner estimate for one execution of the step, where available. |
| `actual_rows` | Total rows emitted by the step across its executions. A scan counts candidates before filtering. |
| `loops` | Number of executions. Zero identifies a reported step that was never run. |
| `actual_rows_per_loop` | Compare this value with `estimated_rows` for a correlated subquery or another repeated step. It is NULL if the step never ran. |
| `elapsed_ms` | Measured elapsed milliseconds accumulated across executions, including waits and work done by children inside the timed operation. |

For example, a scan can return 1,000 candidate rows while a filter emits only 20. A downstream limit can stop pulling rows early, so a step's output count can be smaller than the number of rows it examined. A `JOIN RETRY` marks buffered rows from an abandoned join order. The final `RESULT` row gives the SELECT's output count and elapsed time. Nested timings overlap, so adding step times can exceed the total. A NULL estimate means the planner has no estimate for that step. `ANALYZE table_name` refreshes stored statistics; running `EXPLAIN ANALYZE` measures a query using the current statistics.

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

With ORDER BY, the default frame runs from the partition start through the current row and its peers. Without ORDER BY, it covers the whole partition. Use `ROWS` for row offsets, `GROUPS` for peer-group offsets in sort order, or `RANGE` for distances from one numeric sort key. `NULLS FIRST` and `NULLS LAST` apply per sort column. Aggregate functions accept `FILTER (WHERE predicate)`, including before `OVER`. This query shows each reading next to its previous and cumulative values.

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

Owner's single-SELECT folder results are fully read and saved to a temporary result file before the first page appears. There is no implicit 4,096-row result limit for a simple archive SELECT. The result grid shows 100 loaded rows per page, the total row count and absolute row numbers. Previous, Next and Last page read this saved result without rerunning SQL. The displayed full-load time includes reading, query evaluation and saving all selected rows, and remains unchanged when navigating pages. `EXPLAIN ANALYZE` on a simple archive SELECT also measures the full result unless an explicit limit is given. Sorting and aggregation retain the 128 MiB intermediate memory budget; joins, CTEs and subqueries retain their intermediate-row safeguards. Writable SQL tables retain their 4,096-row storage limit. `TOP`, `LIMIT`, `OFFSET` and `FETCH` accept nonnegative 32-bit integers; explicit limits still bound the result.

Use SQL-standard row limiting for SELECT queries:

```sql
SELECT * FROM Book_3 ORDER BY ID FETCH FIRST 100 ROWS ONLY;
SELECT * FROM Book_3 ORDER BY ID OFFSET 4096 ROWS FETCH NEXT 100 ROWS ONLY;
```

`FIRST` and `NEXT` have the same meaning, as do `ROW` and `ROWS`. Omitting the fetch count means one row. Write `OFFSET` before `FETCH`, and use `ORDER BY` when a stable subset is needed. The UI's page buttons navigate the already loaded result and do not add an OFFSET/FETCH clause or rerun the query. `LIMIT` and `TOP` remain compatibility extensions. See the [SELECT row-limiting syntax and standards notes](https://www.postgresql.org/docs/current/sql-select.html#SQL-LIMIT).
