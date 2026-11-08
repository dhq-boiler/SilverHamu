# catdb

![Status: Experimental](https://img.shields.io/badge/status-Experimental-orange)

catdb is an embedded transactional key-value database written in Rust. It stores ordered byte keys and byte values, so applications can look up one key or read a range from a consistent snapshot. Multiple threads can update the same database, and successful commits are acknowledged after the log is synchronized.

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

Connect to `127.0.0.1:7400` with the generated token. `docker compose down` stops the service and retains its data volume. [Service configuration and client protocol](docs/service.md)

## Benchmark results

In a warm-cache Python client benchmark with 100,000 keys and 256-byte values, four clients produced the following medians across three runs:

| Service | Key lookups/s | Durable updates/s | Update p99 |
|---|---:|---:|---:|
| catdb | 8,047 | 1,133 | 5.08 ms |
| PostgreSQL | 2,554 | 1,298 | 5.55 ms |
| MySQL | 2,436 | 374 | 19.08 ms |
| Oracle Free | 6,026 | 1,280 | 5.37 ms |

In the separate **embedded, single-client** test, catdb completed 303.5 durable updates/s and SQLite completed 596.5. [Comparison charts, versions, and reproduction steps](docs/benchmarks/peer-refresh-2026-11-07/README.md) · [All experiments](docs/benchmarks/)
