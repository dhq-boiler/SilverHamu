# catdb

![Status: Experimental](https://img.shields.io/badge/status-Experimental-orange)

catdb is an embedded transactional key-value database written in Rust. It stores ordered byte keys and byte values, so applications can look up one key or read a range from a consistent snapshot. Multiple threads can update the same database, and successful commits are acknowledged after the log is synchronized.

New databases use a preallocated write-ahead log that checkpoints and grows automatically. Existing databases keep their storage policy on upgrade. [Capacity management and explicit migration](docs/service.md#default-wal-and-upgrades) · [Sustained writes across checkpoints and growth](docs/benchmarks/managed-wal-2026-11-08/README.md)

Use catdb directly from Rust, C, or C#. To share a database across processes, run the optional authenticated TCP service on its own or in Docker. **Experimental:** APIs and file formats may change. Use disposable data when evaluating catdb. [MIT license](LICENSE)

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

## Benchmark results

With a manually sized preallocated WAL, catdb completed **1,882 durable updates/s with four clients** and **4,720/s with sixteen** in the November 8 single-key service comparison. Both throughput and p99 latency beat PostgreSQL 18.6 and Oracle 26ai Free in each of three repetitions on the same host. This measured steady-state updates after initial conversion. [Durability checks, all runs and configuration](docs/benchmarks/durable-redo-2026-11-08/README.md)

An earlier November 8 service comparison measured the append-WAL build against PostgreSQL 18.6 and Oracle 26ai Free. With four clients, catdb completed 1,071 durable updates/s, PostgreSQL 1,280 and Oracle Free 1,172. At sixteen clients the rates were 2,352, 1,542 and 3,664 respectively. catdb led on lookup and 100-row range throughput at both connection counts. [Full results, p99, run-to-run spread and reproduction](docs/benchmarks/grouped-writes-2026-11-08/README.md)

In the November 7 warm-cache Python client benchmark with 100,000 keys and 256-byte values, four clients produced the following medians across three runs:

| Service | Key lookups/s | Durable updates/s | Update p99 |
|---|---:|---:|---:|
| catdb | 8,047 | 1,133 | 5.08 ms |
| PostgreSQL | 2,554 | 1,298 | 5.55 ms |
| MySQL | 2,436 | 374 | 19.08 ms |
| Oracle Free | 6,026 | 1,280 | 5.37 ms |

In the separate **embedded, single-client** test, catdb completed 303.5 durable updates/s and SQLite completed 596.5. [Comparison charts, versions, and reproduction steps](docs/benchmarks/peer-refresh-2026-11-07/README.md) · [All experiments](docs/benchmarks/)
