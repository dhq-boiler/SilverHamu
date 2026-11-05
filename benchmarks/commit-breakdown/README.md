# Commit latency breakdown and single-client collection experiment

This is a warm-cache, closed-loop, Python-client API experiment. It is not a
storage power-loss certification, engine-only ceiling, or a new comparison with
other DBMS products. The default service policy and the opt-in
`single-client-commit` candidate share the same file format, writer lock,
conflict validation, `sync_all` calls and publication ordering.

The candidate uses the service's existing accepted-connection counter. With
exactly one connection it omits the collection and refill delay. At zero or
multiple connections, or in embedded mode without a registered service counter,
the previous collection policy applies. Unauthenticated/idle connections count
conservatively as possible additional clients. A connection arriving immediately
after the check can miss that cohort; it still waits for the writer and its own
durability step. The hint never bypasses validation or persistence.

## Reproduction

Requires Docker Linux containers, Python 3, and the existing comparison client
image. The runner owns uniquely named disposable volumes/networks/containers,
publishes no ports, and refuses an existing output directory. Do not run builds,
tests or other benchmarks alongside timed cohorts.

```sh
docker build -f benchmarks/Dockerfile -t catdb-comparison:local .
docker build -f benchmarks/commit-breakdown/Dockerfile --build-arg CATDB_FEATURES=service -t catdb-commit-before:local .
docker build -f benchmarks/commit-breakdown/Dockerfile --build-arg CATDB_FEATURES=service,lock-timing -t catdb-commit-diagnostic:local .
docker build -f benchmarks/commit-breakdown/Dockerfile --build-arg CATDB_FEATURES=service,single-client-commit -t catdb-commit-after:local .
docker build -f benchmarks/commit-breakdown/Dockerfile --build-arg CATDB_FEATURES=service,single-client-commit,lock-timing -t catdb-commit-after-diagnostic:local .
python benchmarks/commit-breakdown/run.py --variant before=catdb-commit-before:local --variant before-diag=catdb-commit-diagnostic:local --variant candidate=catdb-commit-after:local --variant candidate-diag=catdb-commit-after-diagnostic:local --single-variants before before-diag candidate candidate-diag --profile-variants before-diag candidate-diag --clients 1 4 --workloads update --ops 8000 --reps 3 --out artifacts/commit-breakdown-comparison
python benchmarks/commit-breakdown/report.py artifacts/commit-breakdown-comparison docs/benchmarks/commit-breakdown-2026-11-05
```

Each server starts with 100,000 seeded records and 256-byte values. Server and
client containers each receive two CPUs and 4 GiB. Mode order reverses on the
second repeat; client-count order is shuffled using the same seed for each mode.
Every operation is a one-round-trip `put_commit`; 8,000 attempts per condition,
three repeats, no retries. Each instance verifies every value after initialization,
after each client-count case, and after server restart. Raw latency arrays and
diagnostic records are retained, including outliers. Failed attempts would remain
in the results and abort further execution rather than be silently retried.

## Diagnostic boundaries

Only `lock-timing` builds with `CATDB_DIAGNOSTICS` set attach diagnostics to
`commit`, `put_commit`, and `delete_commit` responses. Each worker holds fixed
thread-local arrays; each queued request/reply carries a fixed-size record.
There is no growing server history for this diagnostic path. The benchmark
collector stores at most `BENCH_OPS` records and counts overflow. The older,
separately enabled group timing history is not enabled here.

All timestamps use the same server-process monotonic clock; zero means a phase
was not reached. The original eight-element `trace` remains compatible:

| Index | `trace` timestamp | `detail` timestamp |
|---:|---|---|
| 0 | Request enqueued | Before pending-queue mutex acquisition |
| 1 | Batch selected | Leader begins collection |
| 2 | Before writer lock | State lock acquired |
| 3 | Writer lock acquired | Before log reservation |
| 4 | Before WAL writes | State lock reacquired after persistence |
| 5 | WAL writes finished | Request transaction dropped; reply ready |
| 6 | Sync call returned | Request caller received reply |
| 7 | Publication finished | Entry into commit |

`costs` contains cumulative validation and frame-construction nanoseconds for
the group. These are nested within preparation, not extra elapsed phases to add
to it. Group timestamps/costs are shared by its requests; entry and reply times
are request-specific. Deduplicate groups before computing group-average cost.
Failures may have incomplete traces, and are not treated as zero-latency success.
Service dispatch clears prior records even if malformed input never enters
commit. Tests cover conflict and injected persistence failure records.

Queue delay ends at `max(collection_start, enqueued)` so requests arriving during
collection are not assigned negative queue waits. Collection includes timer
overshoot and mutex reacquisition. Cleanup/reply time can include earlier replies
in the batch. These are elapsed intervals, not pure CPU or kernel-wait attribution.

Server dispatch starts **after** receiving and parsing JSON, and ends **before**
response serialization and socket writes. Client request time minus dispatch is
therefore a combined request-encoding/transport/server-decode/server-response/
client-decode/scheduling residual. It is **not** a measured network latency.
The client request timer excludes its subsequent diagnostic bookkeeping; the
main operation timer includes it. Instrumented performance is shown separately.

The report validates every successful recorded timestamp, phase ordering,
bounded record count, and the telescoping sum back to each request duration.
Tail breakdown selects the actual slowest 1% of requests in each run and averages
their stages; it never adds unrelated component p99 values. Whole-run means are
request-weighted; group sync means are deduplicated.

## Safety and measurement checks

```sh
cargo test --locked --features service,lock-timing,single-client-commit
cargo test --locked --features service,fixed-log,single-client-commit
cargo build --locked --features service,lock-timing,single-client-commit --bin catdb-server
# Set CATDB_DIAGNOSTICS=1 in the environment for this command:
python benchmarks/commit-breakdown/test_diagnostics.py
python benchmarks/commit-breakdown/test_profile.py
cargo build --locked --release --features service,single-client-commit --lib
python -m unittest discover -s tests -p '*ffi_checks.py'
```

The existing blocked-sync and before/after-sync child-process tests explicitly
exercise the one-client hint in candidate builds. Other suites cover point/range
and same-group conflicts, old snapshots, resource exhaustion and recovery,
checkpoint interactions, partial writes, sync failure and existing APIs.
Actual device power loss is not simulated. See the published result report for
the executed configurations and adoption decision.
