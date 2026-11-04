# catdb

![Status: Experimental](https://img.shields.io/badge/status-Experimental-orange)

**Experimental.** APIs, the network protocol, and file formats may change. catdb is not production-ready or certified for mission-critical use. Keep independent backups and evaluate it with disposable data first.

catdb is a Rust key-value database that runs either **embedded, without a service**, or as an **optional service process**, including in a Docker container. Both modes use the same transaction engine.

## What is implemented

- Ordered byte keys and opaque byte values.
- Optimistic serializable transactions, snapshot reads, and read/write/range conflict detection.
- Multiple concurrent writers with group commit; successful commits are acknowledged after log synchronization.
- Checkpoints, crash recovery, bounded page reads, and configurable engine resource limits.
- A C ABI and source-level C# bindings.
- An optional authenticated TCP service with bounded connections, requests, and transaction lifetimes.
- A non-root Docker image and Compose configuration with persistent storage.

## What is not promised

- No SQL parser, relational query optimizer, replication, or automatic failover.
- No built-in TLS, per-user authorization, or service-side request deduplication.
- No automatic service checkpoint scheduler. Schedule checkpoints before the current 1 GiB WAL limit is reached.
- The transaction engine loads data into memory. Resource budgets are not a hard bound on total process memory or database size during recovery.
- Process-crash tests are not proof against every storage device failure or actual power loss. Durability depends on the operating system and storage honoring synchronization.
- No claim of being faster than SQLite or other databases across workloads.

The service protocol is experimental framed JSON over TCP, **not HTTP or SQL**. The default host binding is loopback. Use an encrypted tunnel or TLS proxy for cross-host access; the shared token alone does not encrypt traffic.

## API microbenchmarks (experimental)

A reproducible comparison of **catdb, SQLite, PostgreSQL, MySQL and Oracle Free**
on one workstation. Embedded and service results are shown separately: the service
measurements include TCP, drivers and an additional client container.

The workload uses **100,000 synthetic records, 256-byte values, 1 or 4 clients**,
with a separate transaction for every lookup, 100-record keyset scan or durable
single-record update. Each condition runs three times in varied order. Bars show
medians; whiskers show the minimum and maximum of the three runs.

![Embedded API throughput and p99 latency comparison](docs/benchmarks/2026-11-05/embedded.svg)

![Service API throughput and p99 latency comparison](docs/benchmarks/2026-11-05/service.svg)

These are **warm-cache Python-client microbenchmarks**, including API and driver
overhead. Python thread scheduling and the GIL can limit four-client throughput.
SQLite is the bundled **3.40.1**, not the latest upstream release.
They are not a general DBMS ranking, an engine-only ceiling, or a comparison of
SQL features, cold startup, larger-than-memory datasets or production reliability.
Faster results in one panel do not imply superiority in the others.

In this run, catdb's embedded single-client reads were faster than the bundled
SQLite path, while service updates lagged PostgreSQL and Oracle Free. All 864,000
operations and 144 full-content checks passed; these checks do not prove power-loss
durability.

See the [results and machine configuration](docs/benchmarks/2026-11-05/README.md),
[complete numerical table](docs/benchmarks/2026-11-05/table.md),
[methodology and reproduction commands](benchmarks/README.md), and
[raw run summaries](docs/benchmarks/2026-11-05/results.json).
SQL Server is not measured because the reviewed edition requires prior written
permission to publish benchmark results; see [product coverage](benchmarks/README.md#product-coverage-and-publication).

## Build

Requires Rust **1.89 or newer**. Python 3 is used by the optional client and integration tests. Docker is needed only for containers.

```sh
git clone https://github.com/dhq-boiler/catdb.git
cd catdb
cargo build --locked --release --lib
```

The package name is `catdb`. The Rust library target, native library name, C ABI prefixes, and C# namespaces retain `newdbms`/`NewDbms` for compatibility with the original implementation. A default embedded build has no third-party runtime dependencies; service dependencies are opt-in.

### Embedded Rust

Use the repository as a Git dependency (pin a commit for reproducibility):

```toml
[dependencies]
newdbms = { package = "catdb", git = "https://github.com/dhq-boiler/catdb.git" }
```

```rust
use newdbms::transactions::{Database, ResourceLimits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // This ID identifies this example database; it is not a credential.
    let db = Database::create_with_limits(
        "example.ndb", *b"catdb-example-01", ResourceLimits::default(),
    )?;
    let mut tx = db.begin()?;
    tx.put(b"hello", b"world")?;
    tx.commit()?;

    let mut reader = db.begin()?;
    assert_eq!(reader.get(b"hello")?, Some(b"world".to_vec()));
    reader.rollback();
    Ok(())
}
```

Use `Database::open_with_limits` to reopen an existing database. Creation refuses to overwrite files. One process owns each database; clones of its handle can be shared between threads. An embedded application and a service cannot directly own the same database simultaneously.

The older `create`/`open` convenience APIs use unlimited engine budgets; prefer the explicit limited variants in applications.

### Standalone service

```sh
python scripts/create-catdb-token.py
cargo build --locked --release --features service --bin catdb-server
mkdir data
./target/release/catdb-server --db data/state.ndb --init --token-file .secrets/catdb-token
```

On Windows, run `target\release\catdb-server.exe` instead. This is a foreground process, not a Windows SCM service installer. The token generator refuses to overwrite an existing credential and never prints the token. `--init` creates a missing database but never replaces an existing one.

See [service configuration and protocol](docs/service.md) for commands, bounds, error handling, and shutdown behavior.

### Docker

```sh
python scripts/create-catdb-token.py  # omit if already generated
docker compose up --build -d
docker compose ps
docker compose down
```

On a Linux host, grant the container's group read access to the file before starting:

```sh
sudo chgrp 10001 .secrets/catdb-token
chmod 640 .secrets/catdb-token
```

Compose publishes only `127.0.0.1:7400`. It uses a named data volume, UID/GID 10001, a read-only root filesystem, dropped capabilities, and a mounted credential file. **`docker compose down` preserves the data; `down -v` deletes the volume.** A volume is persistence, not an independent backup. Do not copy a live database file and assume the result is a consistent backup.

The build context is allowlisted. Local data, credentials, artifacts, and build outputs are not sent into the image.

### Client example

Save these JSON lines as `requests.jsonl`:

```json
{"op":"begin"}
{"op":"put","key":"68656c6c6f","value":"776f726c64"}
{"op":"commit"}
{"op":"begin"}
{"op":"get","key":"68656c6c6f"}
{"op":"rollback"}
```

```sh
python scripts/catdb-client.py --token-file .secrets/catdb-token < requests.jsonl
```

In PowerShell, use `Get-Content requests.jsonl | python scripts/catdb-client.py --token-file .secrets/catdb-token`. Keys and values are hexadecimal byte strings. Authentication is handled by the client. It never retries an uncertain commit automatically.

## Tests

```sh
cargo test --locked --features service -- --test-threads=1
cargo clippy --locked --features service --lib --bin catdb-server -- -D warnings
cargo build --locked --features service --bin catdb-server
python tests/service_protocol.py
cargo build --locked --release --lib
python -m unittest discover -s tests -p "*ffi_checks.py"
```

With Docker available:

```sh
docker build -t catdb:local .
python tests/service_container.py
```

Tests use synthetic data under ignored `artifacts/`. Container tests create and clean up their own containers and volumes. Some Rust tests are ignored worker processes or larger experiments; the parent crash tests invoke the required workers explicitly. See [contributing](CONTRIBUTING.md) for environment requirements.

## Repository layout

| Path | Purpose |
|---|---|
| `src/transactions.rs`, `src/group_commit.rs` | Transaction engine and group commit |
| `src/lib.rs` | Immutable snapshot API |
| `src/service.rs`, `src/bin/catdb-server.rs` | Optional service |
| `src/ffi.rs`, `src/transaction_ffi.rs` | C ABI |
| `dotnet/` | Generic C# bindings |
| `tests/` | Core, ABI, protocol, and container checks |
| `Dockerfile`, `compose.yaml` | Container build and local deployment |

This repository was extracted from the NewDBMS prototype. Application-specific integration, private libraries, benchmark results from personal datasets, and workstation task records are intentionally not part of this distribution.

## License

[MIT](LICENSE). Original copyright notices are retained.
