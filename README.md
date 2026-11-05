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
medians; whiskers show the minimum and maximum of the three runs. These original
product comparisons predate the [group-commit follow-up](#follow-up-faster-four-client-service-updates) below.

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

### Follow-up: faster four-client service updates

The figures above retain the **original catdb implementation**. A new paired
comparison enables group-commit handoff, so requests arriving during a log sync
can share the next sync. Durability acknowledgement and conflict checks are
unchanged; instrumentation is disabled in the chart below.

With the same 100,000 records, 256-byte values and original client, median
four-client update throughput increased from **324 to 630 operations/s (1.94x)**,
while p99 fell from **16.984 to 8.827 ms (48% lower)**. Three runs per condition,
with before/after order alternated; all read conditions are also shown.

![catdb service before and after group-commit handoff, throughput and p99 for 1 and 4 clients](docs/benchmarks/service-update-2026-11-05/service-update.svg)

All 288,000 operations and 48 full-content checks, including six restarts, passed.
These checks do not prove power-loss durability. PostgreSQL and Oracle were not
rerun in this follow-up; this improvement does not establish that catdb overtook
them. See the [full report, timing breakdown and observer overhead](docs/benchmarks/service-update-2026-11-05/README.md),
[numerical table](docs/benchmarks/service-update-2026-11-05/table.md) and
[reproduction commands](benchmarks/service-update/README.md).

### Follow-up: sharing the remaining log-sync cost

A further comparison reruns **catdb before/after, PostgreSQL and Oracle Free**
with the same 100,000 records, 256-byte values and original client, three times
in varied order. A bounded collection policy lets recently active writers
re-form a full group. Four-client throughput improves from **568 to 776
operations/s (37% higher)** and p99 from **9.880 to 7.230 ms
(27% lower)**. Single-client throughput remains approximately
238–239 operations/s.

![Durable service updates: contemporaneous catdb before and after, PostgreSQL and Oracle Free, 1 and 4 clients](docs/benchmarks/log-sync-2026-11-05/service-updates.svg)

**Individual log syncs still take about 3 ms.** The gain comes from sharing more
updates per sync, not weakening durability. PostgreSQL (1183
operations/s) and Oracle Free (1081) remain faster at four clients.
All 192,000 main-run operations and 48 full-content checks, including restarts,
passed. These checks are not proof against actual power loss.

The [full report](docs/benchmarks/log-sync-2026-11-05/README.md) retains rejected
candidates, I/O probes, timing overhead and limitations. See the
[numerical table](docs/benchmarks/log-sync-2026-11-05/table.md),
[raw results](docs/benchmarks/log-sync-2026-11-05/results.json) and
[reproduction commands](benchmarks/log-sync/README.md).

### Follow-up: fixed-length WAL prototype

A fixed-length prototype preinitializes 64 MiB and stores logical-end records on
separate metadata pages. **Both payload sync and end-record sync complete before
success.** It remains opt-in; the default service still uses the append log.

In the same 100,000-record, 256-byte paired benchmark, four-client throughput
changes from **773 to 787 updates/s (1.8% higher)**, but p99 changes
from **7.297 to 7.390 ms (1.3% worse)**. Each mode runs three times;
this is not a convincing four-client tail-latency improvement. Single-client
results, all run ranges and the unfavorable result are included below.

![Current append log versus opt-in fixed-length WAL prototype, throughput and p99 for 1 and 4 clients](docs/benchmarks/fixed-log-2026-11-05/fixed-log.svg)

Steady-state application log writes increase by **6.6%** at four clients;
initial zero filling and extra reserved space are additional costs. This is not
physical-device write amplification. A separate block-I/O comparison measured
**81.5% more server-attributed write bytes** (18.99 vs 10.47 MB per 8,000 updates);
it excludes initialization and is not complete physical-media accounting.
All 96,000 main-run operations and 24
full-content checks, including restarts, passed. The new recovery format remains
Experimental and actual power loss has not been tested.

See the [report, synchronization breakdown and reopen times](docs/benchmarks/fixed-log-2026-11-05/README.md),
[format and recovery design](docs/fixed-log.md),
[raw data](docs/benchmarks/fixed-log-2026-11-05/results.json) and
[reproduction commands](benchmarks/fixed-log/README.md). Earlier product results
are retained; PostgreSQL and Oracle were not rerun for this prototype.

### Follow-up: single-request service APIs

Single-request reads and mutations remove explicit begin/finish network round
trips while using the same transaction engine and durable commit path. Ping now
reads only the committed version instead of computing full database statistics.
The original transaction API remains available, including stable multi-page reads.

In three paired repeats at 100,000 records / 256-byte values, four-client updates
rise from **794 to 1,013 operations/s (27.6% higher)** and p99 falls from
**7.334 to 5.889 ms (19.7% lower)**. The new server's original API is also measured
as a control; all three series, including regressions and repeat ranges, appear
below. Key lookups and 100-row scans are included, not only updates.

![Original server, new server with legacy API, and new single-request API: throughput and p99](docs/benchmarks/single-request-2026-11-05/single-request.svg)

All **432,000 operations and 72 full-content checks**, including nine restarts,
passed. No durability barrier was removed. These remain warm-cache, closed-loop
Python API measurements on one workstation, not a general product ranking.
PostgreSQL and Oracle were not rerun in this follow-up.

See the [full report and ping comparison](docs/benchmarks/single-request-2026-11-05/README.md),
[raw results](docs/benchmarks/single-request-2026-11-05/results.json),
[reproduction commands](benchmarks/single-request/README.md), and
[API semantics](docs/service.md#single-request-operations). In particular,
`scan_latest` pages do not share a snapshot across requests.

### Follow-up: read-only snapshots and bounded reclamation

A further paired comparison uses the same single-request API on both versions.
Read-only transactions avoid write-validation bookkeeping; values are copied
outside the state lock, and old versions are reclaimed in bounded portions.

**The result is a tradeoff, not a general speedup.** At 100,000 records / 256 B,
four-client updates remain near 990 ops/s. Four-client scans improve slightly,
while lookup and single-client scan throughput decline. All 288,000 operations,
48 full-content checks and six restarts pass.

![Service throughput and p99 before and after read-only snapshots and bounded reclamation](docs/benchmarks/read-gc-2026-11-05/read-gc.svg)

In separate instrumented native tests, large-value read p99 improves from
**0.552 to 0.051 ms**, and releasing an old snapshot from **26.086 to 0.066 ms**.
However, complete reclamation increases from **26.086 to 58.599 ms**, its read p99
worsens, and the worst observed read across those repeats increases from
**26.413 to 32.121 ms**. Deferred reclamation also retains more memory temporarily.
No durability barrier is removed; no peer DBMS is rerun.

See the [full tradeoffs, memory results and safety checks](docs/benchmarks/read-gc-2026-11-05/README.md),
[all service values](docs/benchmarks/read-gc-2026-11-05/table.md),
[native diagnostic results](docs/benchmarks/read-gc-2026-11-05/auxiliary/table.md),
and [reproduction commands](benchmarks/read-gc/README.md).

### Follow-up: optional background reclamation

An opt-in worker moves version reclamation off transaction begin/drop. Three
scheduling candidates were measured; the conservative policy is retained and
**disabled by default**, because it trades collection speed for request latency.

In the native old-generation-release workload, read p99 improves from
**0.078 to 0.023 ms**, and the worst read across all three repeats falls from
**32.239 to 1.277 ms**. Complete reclamation instead grows from **55.689 to
319.453 ms**. Post-drain heap and modeled retained data agree; the extra worker's
stack and total process RSS were not measured. Timing-omitted runs retain the
same tradeoff; all native measurements still include allocation accounting.

![Native reclamation read tail and total collection time, including observer comparison](docs/benchmarks/reclamation-2026-11-05/native.svg)

The ordinary 100k-record / 256-B service comparison is mixed: four-client updates
are **1,037 to 1,028 ops/s**, p99 **5.697 to 5.591 ms**, while lookup throughput
and several worst response times worsen. All **288,000 operations, 48 full-content
checks and six restarts** pass. No durability barrier is weakened.

![Service comparison with and without background reclamation](docs/benchmarks/reclamation-2026-11-05/service/reclamation.svg)

See the [complete report, all three candidates and memory/observer limits](docs/benchmarks/reclamation-2026-11-05/README.md),
[worst response times](docs/benchmarks/reclamation-2026-11-05/service/maxima.md),
[reproduction commands](benchmarks/reclamation/README.md), and
[opt-in configuration](docs/service.md#opt-in-background-reclamation).

### Follow-up: investigating the 319 ms reclamation cost

**The combined improvement target was not met.** Profiling attributes 82–86% of
worker wall time to repeated contention pauses. Three prototypes were tested;
none reduced complete reclamation while preserving read p99. The previous
production policy is retained, with background reclamation still off by default.

The fastest prototype reduces median completion **299.091 → 117.192 ms**, but
read p99 rises **0.028 → 0.033 ms** and worst read across three repeats rises
**1.428 → 1.601 ms**. These are paired native measurements, not an adopted
performance improvement. Light-instrumentation and ordinary-service results,
including regressions, are also published.

![Reclamation cost and read-tail tradeoff for the unadopted prototype](docs/benchmarks/reclamation-cost-2026-11-05/native.svg)

All 102 native runs and 288,000 service operations passed their consistency and
reopen checks; GC drained heap matches at 76.641 MiB. Only optional diagnostics
are retained. See the [cause, all candidates, safety and memory limits](docs/benchmarks/reclamation-cost-2026-11-05/README.md),
[service comparison figure](docs/benchmarks/reclamation-cost-2026-11-05/service/reclamation.svg),
and [reproduction commands](benchmarks/reclamation-cost/README.md).

### Follow-up: fewer shared-lock acquisitions

Two snapshot-registration prototypes reduce data-state lock acquisitions from
**three to two per read**, but neither meets the combined latency and completion
goal. **The production implementation remains unchanged.**

The atomic-release prototype reduces profiled reclamation **286.492 → 199.247 ms**
and worst read **1.178 → 1.016 ms**, while read p99 regresses
**0.026 → 0.034 ms**. With lighter instrumentation, reclamation instead grows
**228.955 → 288.662 ms**. Both favorable and unfavorable results are retained.

![Two snapshot-registration prototypes compared with their paired controls](docs/benchmarks/reclamation-pins-2026-11-05/native.svg)

All 72 native runs pass consistency and reopen checks. Drained data charges
match; the prototype registry adds 40 bytes to measured final heap. See the
[report, safety tests, memory limits and service comparison](docs/benchmarks/reclamation-pins-2026-11-05/README.md)
and [reproduction commands](benchmarks/reclamation-pins/README.md).

### Follow-up: preparation outside the reclamation lock

A bounded two-phase prototype captures version metadata, prepares removal counts
outside the shared lock, then validates and applies the plan. **It fails the
light-instrumentation comparison and is not adopted.** Production keeps the
previous collector.

Across three paired 100k-record release runs, read p99 stays approximately
**0.021 ms**, but completion grows **203.396 → 324.006 ms** and worst read
**0.793 → 1.374 ms**. Worst worker lock hold also grows. Final heap matches;
peak requested heap increases by about 9.7 KiB.

![Two-phase reclamation preparation, light acceptance and detailed diagnostics](docs/benchmarks/reclamation-plan-2026-11-05/native.svg)

All 36 native runs and the correctness/recovery suites pass. The longer service
comparison is not rerun after this failed gate. See the [full result and limitations](docs/benchmarks/reclamation-plan-2026-11-05/README.md)
and [reproduction commands](benchmarks/reclamation-plan/README.md).

### Follow-up: reusing leading history keys

A small collector change avoids repeated search and copying for fully reclaimable
leading keys. It reduces detailed timed collection work by **23%**, but does not
meet all acceptance targets. One follow-up combines it with 500 us contention
backoff; the measured light release case improves completion **245.172 →
145.638 ms** and worst read **0.941 → 0.636 ms**, while read p99 increases
**0.021390 → 0.022422 ms**. Mixed-workload worst read also grows
**1.015 → 3.415 ms**.

**Neither condition is adopted; production keeps the previous collector.**
All 72 native runs pass consistency/reopen checks, with identical final heap.

![Leading-key reuse and shorter contention backoff: benefits and regressions](docs/benchmarks/reclamation-front-2026-11-05/half.svg)

See [both cohorts, memory and recovery validation](docs/benchmarks/reclamation-front-2026-11-05/README.md)
and [reproduction commands](benchmarks/reclamation-front/README.md). Service and
other-product comparisons were not rerun after the failed native gate.

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

### Experiment: subtree generations for range validation

A matched count-tree prototype adds each subtree's maximum update generation.
At 1,000,000 rows with updates localized elsewhere, unchanged-half range validation
p99 falls from **22.4 ms to 1.0 µs**, versus the same tree with a global-generation guard.
But update p99 and rank100 latency worsen, and estimated retained node memory rises
**11.1%**. Broad ranges with early conflicts can favor plain traversal.

![Count and generation index: benefits and costs](docs/benchmarks/generation-index-2026-11-05/comparison.svg)

This is a **single-threaded in-memory index experiment with u64 values**, not a
service speedup or persistence change. The tree is not adopted in the engine.
See the [audit of the five original ideas, complete results, tests and limitations](docs/benchmarks/generation-index-2026-11-05/README.md).
A whole-database generation guard can already skip checks when no committed
updates occurred; same-group writes would still need independent validation.

### Follow-up: production generation guard

The production engine now skips committed-state range scans when the transaction
snapshot still equals the database generation. **Same-group earlier writes are
always checked**, and durable acknowledgement is unchanged.

In matched 100k/1M single-writer tests, with three before/after trials per
local/spread distribution, 1M-row range-read-then-update commit p99 falls from
**25.20 to 3.13 ms** (local) and **24.69 to 3.06 ms** (spread). All 24 runs pass
current, old-snapshot and reopened-state full-content checks. This is a targeted
range-validation workload, not a remeasurement of four-client service throughput.

![Production generation guard: validation and durable commit latency](docs/benchmarks/global-generation-2026-11-05/comparison.svg)

Advancing-generation transactions still scan; the 100k/local stale-range p99
worsens from 365 to 820 µs. The [full report](docs/benchmarks/global-generation-2026-11-05/README.md)
includes that result, all-trial maxima, safety checks, raw samples and reproduction
commands. No persistent metadata or file-format change is needed.

### Follow-up: remove redundant copies and point lookups

Three small production changes remove single-request JSON cloning, committed-row
lookups for unchanged-generation point dependencies, and unused export dependencies.
At 1M rows with 10k read dependencies, validation p99 changes **7.962 ms → 45.9 µs**
and durable commit p99 **11.750 → 3.706 ms**. A 256-KiB in-process single-request
put saves **524,947 allocated bytes and nine allocation calls** per operation.

![Work elimination: gains and adverse results](docs/benchmarks/work-elision-2026-11-05/comparison.svg)

Not every latency improves: 1M-row single-key commit p99 worsens **2.379 → 3.142 ms**,
and export has no demonstrated speedup (only one allocation / 194 bytes removed).
Time and allocation use separate builds, with three paired trials per condition.
See the [full results, safety evidence and reproduction steps](docs/benchmarks/work-elision-2026-11-05/README.md)
and [cross-path audit with deferred candidates](docs/benchmarks/work-elision-2026-11-05/audit.md).
These are targeted engine/in-process measurements, not four-client TCP throughput.

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
