# Maximum-latency follow-up

This experiment compares the existing collection policy (`before`) and the
opt-in `single-client-commit` policy (`candidate`). It does not weaken durability
or change the log format. The only new runtime mechanism is an optional bounded
diagnostic snapshot, enabled in `lock-timing` builds with
`CATDB_DIAGNOSTICS=pull`.

## Why a second experiment

The previous three-run comparison had a worse single largest response for the
candidate. Those uninstrumented operations had no trace, so their cause could
not be established from separate instrumented runs. A sample maximum alone is
also unstable when rare stalls occur. This follow-up records every latency and
compares five runs per condition, plus request counts strictly above 10/20/50 ms.

The lightweight collector retrieves the record of the same request immediately
after its operation timer stops, whenever it reaches 10 ms or sets a new maximum
for that worker. Therefore it also captures each run's maximum when all responses
are below 10 ms. It preserves a request index and worker/operation index so the
report can check correspondence with the original latency array.

## Reproduce

Requires Python 3, Docker Linux containers and the comparison client image. Do
not run builds, test suites or other benchmarks during timed cohorts. All data is
synthetic. The runner creates uniquely owned internal networks, volumes and
containers, exposes no host ports, and refuses an existing output directory.

```sh
docker build -f benchmarks/Dockerfile -t catdb-comparison:local .
docker build -f benchmarks/commit-tail/Dockerfile --build-arg CATDB_FEATURES=service -t catdb-tail-before:local .
docker build -f benchmarks/commit-tail/Dockerfile --build-arg CATDB_FEATURES=service,lock-timing -t catdb-tail-before-light:local .
docker build -f benchmarks/commit-tail/Dockerfile --build-arg CATDB_FEATURES=service,single-client-commit -t catdb-tail-candidate:local .
docker build -f benchmarks/commit-tail/Dockerfile --build-arg CATDB_FEATURES=service,single-client-commit,lock-timing -t catdb-tail-candidate-light:local .
python benchmarks/commit-tail/run.py --variant before=catdb-tail-before:local --variant before-light=catdb-tail-before-light:local --variant candidate=catdb-tail-candidate:local --variant candidate-light=catdb-tail-candidate-light:local --single-variants before before-light candidate candidate-light --profile-variants before-light candidate-light --clients 1 4 --workloads update --ops 8000 --reps 5 --out artifacts/commit-tail-comparison
python benchmarks/commit-tail/report.py artifacts/commit-tail-comparison docs/benchmarks/commit-tail-2026-11-05
```

The server and client each have two CPUs and 4 GiB. Each server starts from
100,000 records with 256-byte values. Each of its two conditions executes 8,000
single-request updates, with one or four connections. The same original key and
value generator, seed, worker scheduling, warmup and content oracle are used.
Modes rotate and reverse order across repeats; connection-count cases use the
same shuffled order for each mode in a repeat. The workload equivalence test
checks that the copied worker differs only at the post-sample collection hook.

Every fresh instance verifies all 100,000 records after initialization, after
each condition, and after restart. Failed operations are not retried. The full
experiment has 320,000 updates and 80 full-content checks. The primary
uninstrumented comparison accounts for half those operations; the rest measure
the observer effect. This is a warm-cache Python TCP/API experiment, not an
engine-only throughput ceiling or a general product ranking.

## Bounded pull diagnostics

The authenticated, serial connection retains one fixed-size `CommitDiagnostic`.
No key/value data is copied into this snapshot. Normal commit responses remain
unchanged in pull mode. An explicit `diagnostic_last` request returns it, or null
when no matching previous request exists. The command is available only in pull
mode. Ordinary non-probe requests replace or clear the record; probes themselves
do not change it or increment the request index. Thus repeated probes cannot
accumulate server history, and a failed request cannot reuse an older trace.
Records are local to their connection, including across concurrent requests on
other connections. Authentication, connection limits and transaction expiry
remain in force. Fatal storage errors can close a connection before retrieval;
the collector reports missing/mismatched traces rather than inventing timings.

The old full-response diagnostic mode remains supported. Arrays retain the
[existing phase definitions](../commit-breakdown/README.md#diagnostic-boundaries).
The additional `request_index` counts ordinary post-authentication requests on
that connection (modulo 2^64), while `commit_ok` records whether dispatch succeeded.

Client collection is capped at the number of attempted operations, with an
explicit dropped count. A probe runs **after** that operation's latency sample,
so its own elapsed time does not inflate the sample. However, it delays later
arrivals and is included in whole-run throughput. Instrumentation still samples
server clocks for every commit. This is lower-volume telemetry, not zero-overhead
telemetry. Its observer cohorts are reported separately and are not pooled with
uninstrumented latency distributions.

## Attribution and limits

The report verifies:

- Every probe belongs to the expected worker, operation and server request index.
- Every recorded latency equals that operation's raw latency sample.
- Every worker's maximum and all >10/20/50 ms requests are present.
- Phase ordering, nested cost bounds and sums back to request/operation duration.
- No overflow, missing response, failed probe or silently retried operation.

The protocol-request residual includes request/response encoding, transport,
parsing and scheduling; outer client work is reported separately. The interval
around `sync_all` includes time blocked inside the system call **and** delays
before the thread resumes. It does not establish device I/O versus scheduler
attribution. No ETW/perf capture or physical power-cut test is performed here.

Counts are request counts, not independent disk events: one group can give four
requests the same sync stall. Five run distributions are shown without treating
requests as independent samples for a confidence claim. Uninstrumented outliers
still have no individual trace; instrumented observations cannot prove their
cause retroactively. No fixed observer-overhead correction is subtracted.

## Verification

```sh
cargo test --locked --features service,lock-timing,single-client-commit -- --test-threads=1
cargo build --locked --features service,lock-timing,single-client-commit --bin silverhamu-server
# Set CATDB_DIAGNOSTICS=pull for the next command:
python benchmarks/commit-tail/test_protocol.py
# Set CATDB_DIAGNOSTICS=1 for legacy full-response diagnostic checks:
python benchmarks/commit-breakdown/test_diagnostics.py
python benchmarks/commit-tail/test_collector.py
python benchmarks/commit-tail/test_attribution.py
cargo build --locked --release --features service,single-client-commit --lib
python -m unittest discover -s tests -p '*ffi_checks.py'
```

Coverage includes connection-count transitions, isolated records, failed commits,
stale-record prevention, threshold boundaries, collection bounds, and original
workload equivalence. Existing suites cover same-group/point/range conflicts,
old snapshots, sync-before-success/visibility, storage failures and child-process
termination before/after sync. Published results state the actual executed checks.

Only evidence-supported changes to commit policy are considered. If both policies
show sync outliers without a reproducible added-wait mechanism, the default is
retained and the unresolved storage/scheduling cause is stated explicitly.
