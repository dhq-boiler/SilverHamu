# Reproducible API comparisons

This is an **experimental, client-observed microbenchmark**, not a SQL benchmark,
TPC result, engine-only speed limit, or claim of product equivalence. catdb does
not provide the relational features of the SQL systems in this comparison.

## Run

Requires Python 3 on the host and Docker running Linux containers. No host ports
are published. The runner creates an isolated internal network, fresh named
volumes and containers with unique `catdb-bench-` names, and removes only those
resources after each product/repetition. Do not supply production credentials or
data: synthetic credentials are generated for each disposable fixture.

```sh
docker pull postgres:18
docker pull mysql:8.4
docker pull container-registry.oracle.com/database/free:latest-lite
docker build -f benchmarks/Dockerfile -t catdb-comparison:local .
# Small correctness/restart smoke run, not publishable performance data:
python benchmarks/run.py --out artifacts/comparison-smoke --rows 1000 --ops 100 --reps 1
# Full comparison; output directory must not already exist:
python benchmarks/run.py --out artifacts/comparison-full --rows 100000 --ops 8000 --reps 3
```

Tags can move. Use the image digests in the published `environment.json` to
reproduce a specific run, replacing the tags in `run.py` with those digests.
The Rust compiler and direct Python dependency versions are specified in the
benchmark Dockerfile. This image is a test tool, not the production service image.

## Method

- 100,000 ordered, eight-byte big-endian keys; 256-byte values. SQL systems use a
  single primary-key table with binary columns; SQLite uses `WITHOUT ROWID`.
- Every operation is its own transaction. Uniform random hit lookup; keyset scan
  of exactly 100 consecutive records; or replacement of one existing value with
  a new sequence stamp, followed by durable commit. No inserts, deletes, joins,
  secondary indexes, hot-key contention or mixed workloads are measured here.
- One or four Python threads, one connection/transaction per thread. Update keys
  are partitioned between threads to avoid same-key contention. Different updates
  always carry different values, so repeated no-op writes are not substituted.
- 8,000 attempted operations per case, split evenly between clients. Closed-loop
  requests: each thread waits for completion before sending its next request.
  p50/p95/p99 use nearest-rank successful-operation latency. Failed requests and
  their latencies are recorded separately; there are no request retries. This is
  **not** an open-loop overload/tail-latency test and does not correct coordinated
  omission. Connection setup and 50 lookup warmups per thread are outside timing.
- Bulk loading is outside timing; Oracle uses READ COMMITTED during bulk loading
  to avoid its SERIALIZABLE/deferred-segment-creation restriction on a new table.
  All measured Oracle transactions use SERIALIZABLE.
- Timing includes transaction start, data conversion, request/response, materialized
  results and commit/rollback. Throughput includes completion of all worker tasks.
  It excludes dataset creation, verification, container startup and restart.
- Each case runs three times on freshly created per-product/per-repetition data.
  Product order is forward, reverse, then rotated; case order is deterministically
  shuffled per repetition. All successful updates are checked after each case.
- A complete ordered scan checks every key/value, including the exact final update
  stamp. Missing-key, empty range and last-record boundaries are checked. A container restart with a 60-second stop timeout
  (embedded: close and open in a fresh process) repeats the full
  check. These are **not** power-cut or fault-injection tests.
- Each server gets a 2-CPU quota and 4 GiB memory limit. The client/embedded process
  gets the same quota/limit. Service tests therefore have an additional client
  container; they must not be ranked directly against embedded results. All data
  resides on Docker named volumes, not Windows bind-mounted database files.
- Defaults are retained except the explicit durability, isolation and memory
  settings below. Internal caches and implementation costs are not identical.
  No checkpoint is explicitly requested during timed cases; each engine's normal
  automatic maintenance remains enabled. catdb is checkpointed after bulk load.

## Durability and isolation settings

| Product | Configuration |
|---|---|
| catdb | Release build, group commit enabled, normal sync-before-acknowledgement path; optimistic serializable transactions. The service additionally uses its built-in wake-on-four policy; the C ABI exposes group-commit enablement only. |
| SQLite | WAL, `synchronous=FULL`, 64 MiB page-cache target, 120 s busy timeout; explicit transactions |
| PostgreSQL | `fsync=on`, `synchronous_commit=on`, SERIALIZABLE; other server settings default |
| MySQL / InnoDB | `innodb_flush_log_at_trx_commit=1`, `sync_binlog=1`, SERIALIZABLE; binary log setting recorded |
| Oracle Free | SERIALIZABLE transactions, session `COMMIT_WAIT=WAIT`, `COMMIT_LOGGING=IMMEDIATE`; defaults otherwise |

These settings request durable acknowledgement, subject to OS/storage correctly
honoring synchronization. Isolation implementations are different; selecting the
same SQL isolation name does not prove identical anomaly prevention. Oracle's
edition limits also apply. This comparison does not exercise those differences.

See the official [SQLite synchronization documentation](https://www.sqlite.org/pragma.html#pragma_synchronous),
[PostgreSQL WAL settings](https://www.postgresql.org/docs/18/runtime-config-wal.html),
[MySQL InnoDB settings](https://dev.mysql.com/doc/refman/8.4/en/innodb-parameters.html#sysvar_innodb_flush_log_at_trx_commit)
and [Oracle COMMIT semantics](https://docs.oracle.com/en/database/oracle/oracle-database/26/sqlrf/COMMIT.html).

## Interpretation limits

Python's GIL, drivers, serialization (catdb service uses hex JSON), round trips,
container networking and CPU quotas can limit results. The native catdb adapter
uses ctypes; SQLite uses Python's built-in sqlite3 extension. SQL drivers are
psycopg, PyMySQL and python-oracledb thin mode. They do not have equal overhead.
Four-thread throughput can be lower than one-thread throughput even for a scalable
engine. This benchmark measures those **specific API paths**, not their fastest
possible native clients. Dataset verification warms caches before measurements;
there is no claim about cold startup or datasets larger than memory.

Results from one workstation and three short repetitions are not a universal
ranking or evidence of mission-critical readiness. Charts show the median of the
three independent run summaries and min/max spread, not a pooled percentile or
confidence interval. Raw per-operation samples are available as gzip JSON arrays.

## Product coverage and publication

- Oracle is the **Free** image from Oracle's registry, covered by its
  [Free Use Terms and Conditions](https://www.oracle.com/downloads/licenses/oracle-free-license.html).
  This is not an Enterprise Edition comparison.
- SQL Server is **not measured** in the published suite. Section 5 of the
  [SQL Server 2022 Developer/Express/Evaluation license](https://www.microsoft.com/content/dam/microsoft/usetm/documents/sql-server/sql-server-2022-developer-express-evaluation/retail-packaged/SQLServer2022_SQLServer2022DeveloperExpressEvaluation_English.pdf/)
  requires Microsoft's prior written approval to disclose benchmark results.
  That permission has not been obtained. No zero/estimated bar substitutes for it.

## Validate and draw the published results

```sh
python benchmarks/validate.py docs/benchmarks/2026-11-05
docker run --rm --network none -v "${PWD}:/repo" catdb-comparison:local python /repo/benchmarks/plot.py /repo/docs/benchmarks/2026-11-05
```

The validator independently recomputes percentiles and throughput, verifies source
hashes (allowing Git line-ending normalization and final blank lines), and compares the full-content hashes across all six API paths for every
stage/repetition. The renderer refuses an incomplete or failed comparison.
[Setup failures and corrections](SETUP.md) are retained separately from timings.
