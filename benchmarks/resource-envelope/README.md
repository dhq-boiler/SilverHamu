# Container resource envelope

This experiment measures the **unchanged default SilverHamu service** under Docker
CPU quotas and memory limits. It is not a comparison with other DBMS products,
a cold-cache test, or a claim that a container emulates a dedicated small PC.

## Reproduce

From the repository root, with Linux containers available:

```sh
docker build -f benchmarks/Dockerfile -t catdb-resource:local .
python benchmarks/resource-envelope/test_client.py
python benchmarks/resource-envelope/run.py --out artifacts/resource-full
python benchmarks/resource-envelope/supplement.py --out-root artifacts/resource-diagnostics
```

The default matrix is 100,000 / 1,000,000 rows, 256-byte values, CPU quotas of
1 / 2, memory limits of 512 / 1024 / 2048 MiB, and three repetitions in a seeded
shuffled order. Each successful configuration runs two 8,000-request workloads
at 1,000 scheduled requests/s, reversing workload order in the second repetition.

- `read`: four read connections, point lookups only.
- `mixed`: one reader and four writer connections; 80% point reads and 20%
  independent one-request durable updates. Four connections do not imply four
  simultaneously executing writes, especially at the moderate offered rate.
- Each workload holds an additional old read snapshot. Every successfully updated
  key is checked against its pre-workload value through that snapshot afterward.
- Two queues, each bounded at 64 requests, separate read and write admission.
  Queue-full requests are recorded, never retried. Errors after submission are
  recorded separately and make the case fail; they are not silently treated as
  uncommitted. Latency is scheduled-arrival-to-completion, including client queue
  delay. Reported percentiles describe successful requests; rejection counts
  must be read alongside them. Throughput includes draining the queues.
- The client has its own 2-CPU / 2-GiB cgroup. Server cgroups include the Python
  supervisor. This is client-observed service performance, not engine capacity.
- `--cpus` limits aggregate CPU time, not CPU affinity. `--memory-swap` equals
  `--memory`; `memory.swap.max == 0` and the applied CPU/memory limits are checked.
- A 100-ms observer records cgroup CPU throttling, memory events, total/peak
  memory, anonymous/file memory, swap, and server-process RSS. Its cost is part
  of the limit. RSS and category maxima are sampled, whereas `memory.peak` is a
  cgroup high-water mark. They are not interchangeable. Cgroup peaks include
  lifecycle and helper costs, not only the timed workload.
- Database seeds are created outside the measured limit and copied into fresh
  uniquely named volumes. Loading the existing database is measured; ingestion
  is not. Host cache is not flushed. Shared/copy-warmed page cache can be charged
  outside the server cgroup. This does not demonstrate a cold low-memory host.
- Launch-to-ready includes Docker/client orchestration. The separate process
  ready field is polled (100 ms, or 500 ms without periodic sampling), so it is
  not a precise sub-millisecond initialization measurement.
- Every successful case verifies all values initially, after each workload, and
  after killing only its owned server child with SIGKILL and reopening the same
  database under the same limit. This is process-crash recovery, not power loss.
- Failed startup is retained, including server logs, cgroup memory events and
  Docker exit state. Increasing memory silently to make a matrix case pass is
  prohibited. `--recover-memory` is a separately labelled diagnostic, preserving
  the original failure result and checking the same database at the rescue limit.

For the higher-arrival and observer-cost diagnostic, use `--rows 100000 --cpus 1`
`--memory 512 --rate 5000 --reps 1`, alternate `--sample 0` and `--sample 1` for
three repetitions, and repeat the sampled workload at `--cpus 2`. This uses the
same 8,000 requests; it is a short overload diagnostic, not a steady-state soak.
An isolated OOM/recovery diagnostic uses `--memory 32 --recover-memory 512`.

`report.py --input RESULT_DIRS... --out REPORT_DIR` validates request timestamps,
counts, queue outcomes and resource records before copying compressed raw data
and producing the chart. Neither production durability nor its resource defaults
are changed by these scripts. All Docker resources have unique names; cleanup
targets only resources created by this invocation.

## Test perspectives

The user approved EQ (content), BV (limits and admission), ST (old generations
and restarts), ER (memory failure and recovery), measurement overhead, and
publication of negative outcomes before implementation.

- `test_client.py`: mixed-to-read transitions, old-value checks, exact acknowledged
  values, finite-queue refusal/drain, corrupted content rejection, zero-capacity
  rejection, and empty/single-sample percentiles. Fake storage tests the workload
  oracle; it is not a substitute for real database recovery.
- `run.py`: actual applied limits, full-content checks, old snapshots, child
  process termination/reopen, and diagnostic failure recovery.
- `report.py`: uniqueness/completeness of request records, timestamp order,
  successes/rejections/errors accounting, and absence of swap use.

Docker semantics: https://docs.docker.com/engine/containers/resource_constraints/

## Explicit service budgets

`--image IMAGE` selects a separately built server image. `--retained-bytes N`
passes the positive byte budget to the server, recording it in the run arguments
and each case. Omitting it retains the service default. The observer records the
requested override in each lifecycle command file. To repeat the million-row
configuration experiment (logical retained budget 640 MiB):

```sh
docker build -f benchmarks/Dockerfile -t catdb-resource-config:local .
python benchmarks/resource-envelope/run.py --out artifacts/resource-config-million --rows 1000000 --retained-bytes 671088640 --image catdb-resource-config:local --recover-memory 1024
```

The container memory limit remains independent. A failure rescued at 1 GiB is
still reported as a failure under the original limit. No automatic increase is
performed in production.


## Initial history capacity comparison

`history_compare.py` runs the same workload against two separately built images.
Build the baseline from revision `79b18f0` in a separate checkout as
`catdb-resource-config:local`, and build the candidate checkout with:

```sh
docker build -f benchmarks/Dockerfile -t catdb-history-one:local .
python benchmarks/resource-envelope/history_compare.py --out artifacts/history-one-comparison --before catdb-resource-config:local --after catdb-history-one:local
python benchmarks/resource-envelope/history_report.py --input artifacts/history-one-comparison --out docs/benchmarks/history-one-2026-11-06
```

Three rounds use before/after, after/before, before/after order. Each image gets
the same four million-row CPU/memory cells (1/2 CPUs, 512/1024 MiB), plus a
100k-row default-budget control at 1 CPU / 512 MiB. The million-row retained
budget is 640 MiB. Each invocation uses the same shuffled condition order and
read-then-mixed workload order. Runs are sequential; the report preserves every
OOM and its separately labelled 1-GiB rescue. It reports update-only p99 from
the mixed workload separately from read-only p99. The generator validates run
completion, full-content checks, request records and failure classification.

This is an initial-allocation study. The mixed workload updates 1,600 distinct
keys while holding an old snapshot; it does not establish steady-state memory
after rewriting the entire database. History-capacity expansion, pinned
snapshots through deletion/reinsertion and budget refusal are also covered by
`tests/resource_limits.rs`. Existing persistence and conflict tests remain in
the regression suite. No service budget, file format or durability setting is
relaxed by the candidate.


## History capacity after aging

The `history-capacity-bench` binary uses the actual persistent Rust database and
opt-in `history-metrics` observation. It is an embedded diagnostic, not a service
throughput benchmark. Production defaults, GC, WAL format and budgets are unchanged.
The optional `allocation-metrics` feature counts live bytes requested through
Rust's global allocator in this binary only; it does not measure malloc usable
size or all process allocations. History scans hold the DB lock and are outside
the response-time windows. Do not poll them on a production request path.

```sh
docker build -f benchmarks/resource-envelope/history-capacity.Dockerfile -t catdb-history-capacity:local .
python benchmarks/resource-envelope/history_capacity_run.py --out artifacts/history-capacity-pilot --rows 100000 --observer both
python benchmarks/resource-envelope/history_capacity_run.py --out artifacts/history-capacity-million --rows 1000000 --observer both --reps 3 --check512
```

Start the million-row comparison only if the pilot can distinguish live history
storage, spare capacity, and RSS after release. Each phase has a 180-second child
deadline; GC has a 60-second deadline. Each case owns a new Docker volume and
containers, removed after records are copied. A failed 512-MiB reopen is retained
as a failed phase, followed by verification/checkpoint at 1 GiB and a separate
512-MiB reopen of the checkpoint. It is never silently counted as a successful
aged-log reopen.

Spread aging updates 25/50/100% of rows in 1,000-row transactions, draining GC
after each batch. Hot aging updates the first 1% of rows sixteen times with an
old reader pinned. This state preparation is separate from response-time samples:
1,000 point reads and four closed-loop writers with 100 commits each. Those four
writers rewrite keys 0..3 to their already expected values; they can further grow
those four histories, which subsequent capacity snapshots include. These windows
are not comparable to fixed-arrival network/service benchmark p99 values.

Counting/scanning is off/on in the paired builds, with reversed order in even
rounds. The common 100-ms cgroup observer remains active in both builds; its own
overhead is not isolated here. Seed creation uses 2 CPUs / 4 GiB outside measured
phases; aging/reopen uses 1 CPU / 1 GiB. The explicit retained-data budget is
640 MiB, and swap is disabled. File-cache charge, RSS and live requested bytes
are recorded separately. These are warm-host-cache container measurements.

The final Linux `malloc_trim(0)` is a diagnostic intervention after the latency
windows. Unchanged live requested bytes with falling RSS demonstrates releasable
allocator-held pages; it does not identify every remaining byte or propose trim
for production. Full verification runs again afterward.
