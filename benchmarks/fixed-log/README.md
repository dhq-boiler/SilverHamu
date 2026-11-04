# Fixed-length WAL comparison

This compares the current durable service (`1c046f2`, also the runtime at
`cd6df02`) with the opt-in fixed-length log in `06bd70b`. It uses the exact
original `benchmarks/runner.py` operations and frozen Python client image.
Read the [format and recovery design](../../docs/fixed-log.md) before enabling it.

## Reproduce

Run from this repository. Docker Desktop/Linux and host Python are required.
Each run creates fresh named volumes and private networks; existing output paths
are refused. Fixtures contain only generated data. Each client/server is limited
to 2 CPUs and 4 GiB. The non-root server owns the fixture directory so it can
create checkpoint replacements. Never use real libraries for these commands.

```sh
git worktree add --detach artifacts/fixed-current cd6df02
docker build -f artifacts/fixed-current/benchmarks/service-update/Dockerfile -t catdb-sync-cohort:local artifacts/fixed-current
git worktree add --detach artifacts/fixed-prototype 06bd70b
# The benchmark Dockerfile can build the pinned runtime source directory.
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" -t catdb-fixed-log:local artifacts/fixed-prototype
git worktree add --detach artifacts/fixed-client 1c22601
docker build -f artifacts/fixed-client/benchmarks/Dockerfile -t catdb-comparison:local artifacts/fixed-client
python benchmarks/fixed-log/run.py --variant current=catdb-sync-cohort:local --variant fixed=catdb-fixed-log:local --fixed-variants fixed --clients 1 4 --workloads update --reps 3 --out artifacts/fixed-rerun
python benchmarks/fixed-log/validate.py artifacts/fixed-rerun
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/fixed-log/plot.py artifacts/fixed-rerun
```

The final prototype has 64 MiB of initialized data capacity and two separately
aligned 4 KiB metadata pages. Both data and end-record syncs finish before
success. It does not substitute asynchronous commits or skip a durability barrier.
The default service remains on the current append log.

There are 100,000 records, 8-byte keys, 256-byte values and 8,000 single-row update
transactions per condition. Runs reverse variant order in repeat 2. The 1/4-client
order is shuffled identically for both modes. Connections and point reads warm
outside timing; begin/put/commit plus Python driver/TCP overhead are all timed.
There is no retry. Initialization, full-value verification and restart are outside
the operation timer. Every condition and reopen verifies all values and empty/end
boundaries, and paired content hashes must agree.

## Bytes and reopen time

`application_written_bytes` counts transaction-frame bytes plus 20 sentinel bytes
and 64 end-record bytes per fixed-log commit group. Frames are 317 bytes for this
specific workload. It is derived from validated logical-end/generation changes
before and after a case; every successful fixed commit group advances generation
once. Counters are read outside timed operations.

This is not physical-device write amplification. `/proc/1/io` is absent; an
initially empty cgroup `io.stat` was later verified to populate after writes.
The supplemental run below records those block counters separately. Initial zero filling and checkpoints
are excluded from steady-state byte counts; reserved file size and the initial
64 MiB write cost are disclosed separately. End records occupy distinct pages,
even though each application write changes only 64 bytes of its page.

After both cases, the server is stopped. A fresh `reopen-bench` process from the
same prototype image opens either format, measures only `Database::open`, and
reports row count, generation and logical bytes. Docker startup/shutdown and
stats collection are outside that timer. The original server then starts again
and the full-content verifier runs. These are warm-cache common-reader open
times, not cold startup or application readiness measurements. Clean fixed-log
open currently performs two repair syncs.

## Timing breakdown and observer overhead

```sh
docker build -f artifacts/fixed-current/benchmarks/service-update/Dockerfile --build-arg CATDB_FEATURES=service,lock-timing -t catdb-sync-cohort-diag:local artifacts/fixed-current
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" --build-arg CATDB_FEATURES=service,fixed-log,lock-timing -t catdb-fixed-log-diag:local artifacts/fixed-prototype
python benchmarks/fixed-log/run.py --variant current=catdb-sync-cohort:local --variant current-diag=catdb-sync-cohort-diag:local --variant fixed=catdb-fixed-log:local --variant fixed-diag=catdb-fixed-log-diag:local --fixed-variants fixed fixed-diag --profile-variants current-diag fixed-diag --fixed-metrics-variants fixed-diag --clients 4 --workloads update --reps 3 --out artifacts/fixed-rerun/observer
python benchmarks/fixed-log/validate.py artifacts/fixed-rerun/observer
python benchmarks/service-update/analyze.py artifacts/fixed-rerun/observer
```

Diagnostic builds record all existing stages and fixed-log payload/end sync sums
and counts. Observer overhead includes server timing, response fields and client
collection; it can alter batch formation. Main-chart timings have it disabled.
Stage percentiles cannot be added together. An uncertain commit or operation
failure stops the run and remains in the output; it is never automatically retried.

The results do not cover actual power cuts, other filesystems, cold cache,
larger-than-memory data, sustained log exhaustion, mixed reads or concurrent
checkpoints. Three repeats are descriptive ranges, not confidence intervals.
Previous PostgreSQL/Oracle results are retained but are not rerun in this experiment.

## Supplemental block-I/O accounting

```sh
python benchmarks/fixed-log/io_run.py --variant current=catdb-sync-cohort:local --variant fixed=catdb-fixed-log:local --fixed-variants fixed --clients 4 --workloads update --reps 3 --out artifacts/fixed-rerun/io-accounting
python benchmarks/fixed-log/validate.py artifacts/fixed-rerun/io-accounting
python benchmarks/fixed-log/report.py artifacts/fixed-rerun
```

This repeats the same 8,000 updates at four clients, three times per mode.
`io_run.py` sums `wbytes`/`wios` across devices in the server container's cgroup
`io.stat`, before and after each case, waiting for two identical samples at least
100 ms apart (bounded to 20 attempts) outside operation timing. It also records
server initialization/conversion counters before the timed updates. The shared
client-side seeding is outside that server cgroup.

These are kernel-attributed block bytes and requests, not complete physical-media
write amplification: some filesystem/journal/controller work may be attributed
elsewhere. All supplemental timing distributions and content checks are retained;
they do not replace the predeclared main comparison.
