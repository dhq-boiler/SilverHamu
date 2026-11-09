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

An administrator can run `GRANT UPDATE ON public_books TO editor` and later `REVOKE UPDATE ON public_books FROM editor`. An embedded host supplies the authenticated role to `ExecuteRoleAsync(sql, role)`. On the token-based SQL service, configure the role and enable `--allow-role-writes` to use these grants. The read-only APIs continue to accept only view queries. For an updatable filtered view, UPDATE and DELETE select only visible rows. INSERT uses the exposed columns and the underlying table's defaults and constraints.
