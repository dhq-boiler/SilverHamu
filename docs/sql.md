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
| Data definition and updates | CREATE/DROP TABLE, INSERT, UPDATE, DELETE, ALTER TABLE ADD COLUMN / RENAME COLUMN |
| Constraints | NOT NULL, UNIQUE, single or composite primary keys, single-column foreign keys, CHECK |
| Indexes | Single-column and composite indexes, leading equality and range predicates, REINDEX for rebuilding older indexes |
| Queries | SELECT, WHERE, ORDER BY, LIMIT/OFFSET, inner and outer joins, GROUP BY, COUNT(*), SUM, AVG |
| Reusable queries | Read-only views, non-correlated single-column IN subqueries, parameterless stored procedures |
| Access control | GRANT SELECT / REVOKE SELECT on a named view, for use with the restricted view-query API |

SQL tables have at most 64 columns and 4,096 rows. Each intermediate join is bounded at 4,096 rows before result pagination. These are SQL-layer limits; the native key-value store has its own resource limits. Physical power-loss qualification remains a separate hardware test.
