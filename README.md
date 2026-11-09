# catdb

![Status: Experimental](https://img.shields.io/badge/status-Experimental-orange)

## Feature matrix

| Capability | catdb | Windows Owner | Scope in this checkout |
|---|:---:|:---:|---|
| Open a local database without a server | Yes | Yes | One owning process per file. Rust, C and C# can embed the storage engine. |
| Transactions and crash recovery | Yes | Yes | Snapshot reads, conflict detection, commit and rollback use the same native engine. |
| SQL tables, indexes and constraints | Yes | Yes | The .NET 10 SQL layer supports primary and foreign keys, checks and composite indexes. SQL tables are limited to 4,096 rows each. |
| Joins, views and aggregates | Yes | Yes | Inner and outer joins support up to eight inputs and 4,096 intermediate rows per join step. |
| [Standard SQL syntax](#sql-syntax-matrix) | Partial | Partial | The matrix below labels individual forms. TODO: Implement missing forms and add conformance tests. |
| Query execution plans | No | No | TODO: Build an explicit plan for each query and expose it through `EXPLAIN` so users can inspect scans, index lookups and joins. |
| Cost-based query optimizer | No | No | TODO: Estimate row counts and compare alternative access paths and join orders to choose an execution plan. |
| Serve authenticated network clients | Yes | No | The Rust service supports shared tokens and TLS accounts. Server-side SQL requires a separately configured SQL host. |
| Back up and restore a standalone database | Yes | Yes | Offline backup preserves the physical file. Embedded online backup captures a committed generation. Account-enrolled services require a coordinated backup. |
| SQL editor and result grid | No | Yes | The standalone SQL CLI is also available for scripts. |
| Replication and automatic failover | No | No | Single-owner storage; no multi-node promotion protocol. |

[Try embedded SQL](dotnet/README.md#embedded-sql-and-cli) · [SQL and service compatibility](docs/sql.md) · [Back up and restore](docs/backup.md)

### SQL syntax matrix

These labels cover common SQL forms in the shared `Catdb.Sql` engine used by catdb and Windows Owner. Each label applies to the form shown. In Owner, a folder opened through `.catlink` is read-only and accepts queries against its tables.

<details>
<summary>Queries, expressions and joins</summary>

| Syntax | Status | Scope |
|---|:---:|---|
| `SELECT *`, `SELECT t.*`, `SELECT col AS alias` | ✓ Supported | Up to 64 result columns. |
| `SELECT DISTINCT` | ✗ Unsupported | |
| `WHERE` with comparisons, `AND`, `OR`, parentheses | ✓ Supported | Compare columns and literals. |
| `IS NULL`, `IS NOT NULL` | ✓ Supported | Example: `SELECT * FROM Book_3 WHERE PublishDate IS NOT NULL`. |
| `LIKE` | ✓ Supported | Text patterns with `%` and `_`. |
| `BETWEEN`, `IN (value, ...)`, unary `NOT` | ✗ Unsupported | |
| `WHERE col IN (SELECT col FROM t ...)` | ✓ Supported | One non-correlated subquery as the whole predicate. Its input is a single table or view. |
| Correlated subqueries, scalar subqueries, `EXISTS`, `FROM (SELECT ...)` | ✗ Unsupported | |
| `INNER JOIN`, `LEFT`, `RIGHT` and `FULL OUTER JOIN` | ✓ Supported | Each `ON` compares two qualified columns for equality. Up to eight inputs. |
| `CROSS JOIN`, `NATURAL JOIN`, `JOIN ... USING` | ✗ Unsupported | |
| `GROUP BY` | ✓ Supported | One or more grouping columns. |
| `HAVING` | ✗ Unsupported | |
| `COUNT(*)` | ✓ Supported | Counts rows after filtering. |
| `COUNT(col)`, `COUNT(DISTINCT col)` | ✗ Unsupported | |
| `SUM(col)`, `AVG(col)` | ✓ Supported | Numeric columns. |
| `MIN(col)`, `MAX(col)` | ✗ Unsupported | |
| `ORDER BY col ASC/DESC` | ✓ Supported | Multiple sort columns. |
| `OFFSET n ROWS FETCH NEXT n ROWS ONLY` | ✗ Unsupported | Available alternatives are `LIMIT n OFFSET m` and `TOP n`. |
| `UNION`, `UNION ALL`, `INTERSECT`, `EXCEPT` | ✗ Unsupported | |
| `WITH`, `WITH RECURSIVE`, window functions (`OVER`) | ✗ Unsupported | |
| `CASE`, `CAST`, arithmetic such as `SELECT price * qty` | ✗ Unsupported | |

</details>

<details>
<summary>Writes, schema, privileges and transactions</summary>

| Syntax | Status | Scope |
|---|:---:|---|
| `INSERT INTO ... VALUES` | ✓ Supported | Optional column list and multiple rows of literals. |
| `INSERT INTO ... SELECT`, `MERGE` | ✗ Unsupported | |
| `UPDATE ... SET col = literal [WHERE ...]` | ✓ Supported | One or more column assignments. |
| `UPDATE ... SET qty = qty + 1` | ✗ Unsupported | Assignment values must be literals. |
| `DELETE FROM ... [WHERE ...]` | ✓ Supported | |
| `CREATE TABLE`, `DROP TABLE` | ✓ Supported | Up to 64 columns per table. |
| `CREATE TABLE ... AS SELECT` | ✗ Unsupported | |
| `NOT NULL`, column-level `UNIQUE`, `DEFAULT literal`, `CHECK` | ✓ Supported | Column and table checks use the supported predicate syntax. |
| Single-column or composite `PRIMARY KEY` | ✓ Supported | |
| Single-column `FOREIGN KEY ... REFERENCES` | ✓ Supported | |
| Composite foreign keys, `ON DELETE CASCADE`, `ON UPDATE CASCADE` | ✗ Unsupported | |
| `ALTER TABLE ... ADD COLUMN`, `RENAME COLUMN` | ✓ Supported | |
| `ALTER TABLE ... DROP COLUMN`, `ALTER COLUMN` | ✗ Unsupported | |
| `CREATE VIEW ... AS SELECT`, `DROP VIEW` | ✓ Supported | Read-only views over base tables. |
| `INSERT`, `UPDATE` or `DELETE` through a view | ✗ Unsupported | |
| `GRANT SELECT ON view`, `REVOKE SELECT ON view` | ✓ Supported | Named roles on published views. |
| `GRANT INSERT`, `GRANT UPDATE`, `GRANT DELETE` | ✗ Unsupported | |
| `COMMIT`, `ROLLBACK` | ✓ Supported | Open a transaction with `BEGIN` or `BEGIN TRANSACTION`. |
| `START TRANSACTION`, `SAVEPOINT`, `ROLLBACK TO SAVEPOINT` | ✗ Unsupported | |

</details>

catdb is an embedded transactional key-value database written in Rust. It stores ordered byte keys and byte values, so applications can look up one key or read a range from a consistent snapshot. Multiple threads can update the same database, and successful commits are acknowledged after the log is synchronized.

New databases use a preallocated write-ahead log that checkpoints and grows automatically. Existing databases keep their storage policy on upgrade. [Capacity management and explicit migration](docs/service.md#default-wal-and-upgrades) · [Sustained writes across checkpoints and growth](docs/benchmarks/managed-wal-2026-11-08/README.md)

Use catdb directly from Rust, C, or C#. To share a database across processes, run the optional authenticated TCP service on its own or in Docker and connect with a [client library](#client-libraries). **Experimental:** APIs and file formats may change. Use disposable data when evaluating catdb. [MIT license](LICENSE)

## Quick start

With Rust 1.89 or newer, run the embedded example:

```sh
git clone https://github.com/dhq-boiler/catdb.git
cd catdb
cargo run --locked --release --example basic
```

The example creates `example.ndb` in the current directory, commits `hello → world`, and reads the value back. Creation requires a new file path. To use catdb in your own Rust project, add the dependency below and follow the [example source](examples/basic.rs). The Rust library name is `newdbms`.

```toml
[dependencies]
newdbms = { package = "catdb", git = "https://github.com/dhq-boiler/catdb.git" }
```

To try the service instead, run these commands from the cloned repository with Python 3 and Docker Compose installed:

```sh
python scripts/create-catdb-token.py
# On a Linux host, allow the container group to read the token:
# sudo chgrp 10001 .secrets/catdb-token && chmod 640 .secrets/catdb-token
docker compose up --build -d
```

Connect to `127.0.0.1:7400` with the generated token. For individual logins and per-database permissions, see [TLS service accounts](docs/accounts.md). `docker compose down` retains the data volume. [Service configuration and client protocol](docs/service.md)

## Client libraries

Start a local catdb server with the [quick start](#quick-start). Choose a language below for an example that connects to the server, writes a binary key, and reads its value back.

| Language and example | Runtime | Install version 0.1.0 | Package |
| --- | --- | --- | --- |
| [C#](https://www.nuget.org/packages/Catdb.Client/0.1.0#readme-body-tab) | .NET 10 | `dotnet package add Catdb.Client --version 0.1.0` | [NuGet](https://www.nuget.org/packages/Catdb.Client/0.1.0) |
| [Go](clients/go/README.md) | Go 1.22+ | `go get github.com/dhq-boiler/catdb/clients/go@v0.1.0` | [Go packages](https://pkg.go.dev/github.com/dhq-boiler/catdb/clients/go@v0.1.0) |
| [Java](clients/java/README.md) | Java 17+ | Gradle `implementation("io.github.dhq-boiler:catdb-client:0.1.0")` · [Maven POM](clients/java/README.md) | [Maven Central](https://central.sonatype.com/artifact/io.github.dhq-boiler/catdb-client/0.1.0) |
| [JavaScript / TypeScript](clients/node/README.md) | Node.js 18+ | `npm install catdb-client@0.1.0` | [npm](https://www.npmjs.com/package/catdb-client/v/0.1.0) |
| [Python](clients/python/README.md) | Python 3.10+ | `python -m pip install catdb-service-client==0.1.0` | [PyPI](https://pypi.org/project/catdb-service-client/0.1.0/) |
| [Ruby](clients/ruby/README.md) | Ruby 3.1+ | `gem install catdb-client -v 0.1.0` | [RubyGems](https://rubygems.org/gems/catdb-client/versions/0.1.0) |

## Back up and restore a database

Close the standalone database or stop its server. From the repository root, build the CLI and use new filenames for the archive and the restored database.

```sh
cargo build --locked --release --bin catdb
./target/release/catdb backup state.ndb state.catbak
./target/release/catdb restore state.catbak recovered.ndb
```

The archive omits all-zero blocks. In the benchmark with unused WAL capacity, a 128.25 MiB database produced a 2.32 MiB archive. Restore verifies the archive and writes the full database, including its reserved space. Services with TLS accounts require a coordinated copy of the data files and account catalog. [Backup, restore and Docker instructions](docs/backup.md) · [Windows and Linux measurements](benchmarks/backup-restore/README.md)

## Benchmark results

### Durable single-key updates

Across 4, 16, 32 and 64 clients, catdb completed more synchronous updates per second and had lower p99 latency than Oracle 26ai Free and PostgreSQL 18.6 in all three trials at each client count. At 32 clients, catdb reached **5,627 updates/s**, 61% more than Oracle, the faster peer. Doubling to 64 clients brought **5,654 updates/s** (+0.5%), while p99 rose from **11.60 to 25.97 ms**.

![Median throughput and p99 latency for three databases. At 4, 16, 32 and 64 clients, catdb completed 1,885, 4,391, 5,627 and 5,654 updates/s, with p99 of 3.25, 5.64, 11.60 and 25.97 ms. The linked results table gives the values for all three databases.](docs/benchmarks/high-clients-2026-11-08/comparison.png)

Each trial timed 8,000 single-key updates against 100,000 keys with 256-byte values, including the request, durable commit and response. Each server and client had 2 CPUs and 4 GiB on the same host. catdb used a 64 MiB preallocated WAL and a connection limit of 64. All timed updates fit before a checkpoint. [Results table, trial ranges and reproduction](docs/benchmarks/high-clients-2026-11-08/README.md)

### Sustained writes with the product defaults

A separate catdb workload with 2 KiB values included automatic checkpoints and growth. The original 4- and 16-client runs sustained **1,728** and **4,022 updates/s**. With 32 and 64 writers, the v0.1.0 release sustained **4,146** and **4,210 updates/s**, with p99 of **25.46** and **36.96 ms**, respectively. All six high-concurrency runs recovered every key after a forced stop. [4- and 16-client trials](docs/benchmarks/managed-wal-2026-11-08/README.md) · [32- and 64-client trials](docs/benchmarks/high-clients-2026-11-08/README.md#automatic-checkpoints-and-growth)

<details>
<summary>Earlier comparisons and embedded results</summary>

- [November 8 · Original 4- and 16-client comparison of preallocated and append WALs](docs/benchmarks/durable-redo-2026-11-08/README.md)
- [November 8 · Append-WAL service, including key lookups and 100-row ranges](docs/benchmarks/grouped-writes-2026-11-08/README.md)
- [November 7 · Multi-product service comparison and embedded SQLite results](docs/benchmarks/peer-refresh-2026-11-07/README.md)
- [All benchmark studies](docs/benchmarks/)

</details>
