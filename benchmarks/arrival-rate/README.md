# Equal-arrival-rate commit experiment

This experiment checks whether the sole-client collection-wait bypass has worse
tails merely because a completion-driven client sends faster and causes more
syncs. `before` uses the default collection policy. `candidate` enables the
existing opt-in `single-client-commit` feature. It skips collection/refill waits
only while exactly one authenticated service connection is active. Validation,
locking, publication, synchronization, and the log format are unchanged.

## Workload and timing contract

- 100,000 synthetic records, 256-byte values, 8,000 offered updates per case.
- One connection at 200/250 requests/s; four at 400/800 requests/s, total.
- Three repeats; modes rotate/reverse and cases shuffle identically per repeat.
- Two CPUs and 4 GiB each for server and Python client containers. Warm cache,
  framed JSON/TCP, separate `put_commit` transaction per update.
- The original seeded random keys are partitioned by worker, so distinct workers
  do not update the same key. Safety/conflict tests cover contention separately.
- A producer schedules absolute deadlines independent of previous completion.
  Missed deadlines remain missed: neither the schedule nor the reported start
  time is shifted forward to conceal backlog.
- One bounded FIFO per connection, 64 pending slots in total, plus 1/4 operations
  in flight. Admission to a full worker FIFO is rejected and counted. No retries
  or redistribution to other connections. All admitted requests drain before exit.
- `planned_ns`: absolute intended time. `admitted_ns`: producer's admission
  attempt. `sent_ns`: worker starts preparing/sending the operation, **not**
  packet arrival at the server. `finished_ns`: response processing completes.
  All four use one client-process monotonic clock.
- **Response** includes planned-to-finished queue/scheduling delay; **operation**
  is sent-to-finished. Producer lag and client FIFO delay are also separated.
  Thresholds count durations strictly greater than 10/20/50 ms. Rejected requests
  remain in the raw events and denominator, not in successful latency quantiles.
- Full content, order, empty/end boundaries are checked initially, after every
  case and after restart: 60 full checks over 12 fresh instances. Only successful
  updates alter the expected-state oracle. Failed requests are retained and stop
  the experiment; capacity rejections do not. Old snapshots, conflicts and crash
  points are covered by the separate existing engine/protocol suites.

## Frequency is measured, not assumed

`benchmark-metrics` adds an authenticated `benchmark_counters` command only to
explicitly enabled builds. It exposes the existing process-local count of commit
sync attempts and the current committed version. The two fields are independent
reads, **not an atomic state snapshot**, and a sync attempt is not necessarily a
successful commit. Counters reset on reopen. Reads occur on worker zero's existing
connection before the ready barrier and after every worker has drained; there
are no writes in flight then. No extra connection disables the sole-client path.
The feature adds no per-commit instrumentation; the engine already maintains the
counter. Default builds reject this optional command.

Each result reports completed requests/s, sync attempts/s and updates/sync.
Raw events and 100 ms / 1 s bins show whether actual sends match the intended
frequency. Equal **offered** rates do not establish equal instantaneous sends or
sync cadence. At four connections, staggered arrivals can change batch formation
even when total offered rates are equal. These tests are rate-limited workloads,
not throughput-ceiling measurements or direct comparisons to older closed-loop
figures.

## Observer cohorts

Normal cohorts have commit tracing disabled. Separate `*-light` cohorts at
250/800 requests/s use `lock-timing` and `CATDB_DIAGNOSTICS=pull`. A connection
retains one fixed diagnostic record. After the operation timer, the client fetches
it when response latency reaches 10 ms or either operation/response sets a new
worker maximum. Thus every successful maximum and threshold exceedance has a
matching record, capped at one record per operation. Indices, timing order and
phase sums are checked; missing or mismatched traces fail the report.

Probes occur **after** the sampled operation but delay subsequent queued work.
When a backlog persists, nearly every request may qualify: this can amplify
backlog and rejection. The observer is not a zero-overhead substitute for the
primary cohort. Report all observer regressions and probe counts separately.
Server/client origins differ, so only durations are combined across processes.
Elapsed log sync includes OS/filesystem/device work and rescheduling; it does
not isolate disk hardware time. Maxima in observer runs cannot explain a different
uninstrumented request retrospectively.

## Reproduce

Use Docker Linux containers and Python 3. All data is disposable. The runner owns
unique internal networks, containers and volumes and opens no host ports. Existing
output directories are refused. Do not run builds/tests/other benchmarks during
timed cohorts. The short pilot is retained separately and excluded from results.

```sh
docker build -f benchmarks/Dockerfile -t catdb-comparison:local .
docker build -f benchmarks/arrival-rate/Dockerfile --build-arg CATDB_FEATURES=service,benchmark-metrics -t catdb-rate-before:local .
docker build -f benchmarks/arrival-rate/Dockerfile --build-arg CATDB_FEATURES=service,benchmark-metrics,lock-timing -t catdb-rate-before-light:local .
docker build -f benchmarks/arrival-rate/Dockerfile --build-arg CATDB_FEATURES=service,benchmark-metrics,single-client-commit -t catdb-rate-candidate:local .
docker build -f benchmarks/arrival-rate/Dockerfile --build-arg CATDB_FEATURES=service,benchmark-metrics,single-client-commit,lock-timing -t catdb-rate-candidate-light:local .
python benchmarks/arrival-rate/run.py --variant before=catdb-rate-before:local --variant before-light=catdb-rate-before-light:local --variant candidate=catdb-rate-candidate:local --variant candidate-light=catdb-rate-candidate-light:local --profile-variants before-light candidate-light --ops 8000 --reps 3 --out artifacts/arrival-rate-comparison
python benchmarks/arrival-rate/report.py artifacts/arrival-rate-comparison docs/benchmarks/arrival-rate-2026-11-05
python benchmarks/arrival-rate/test_schedule.py
python benchmarks/arrival-rate/test_report.py
cargo build --features service,benchmark-metrics --bin catdb-server
# Set CATDB_EXPECT_METRICS=1 for this enabled-feature binary:
CATDB_EXPECT_METRICS=1 python benchmarks/arrival-rate/test_metrics.py
```

The default command uses all four rate cases and only the higher rates for
observer cohorts. `--cases` and `--observer-cases` accept `connections:rate`.
The report expects the completed three-repeat study; partial results remain
available as JSON if a run fails. The offered-arrival schedule adds a producer
thread and timestamps relative to the older client. Observer comparisons measure
the extra diagnostic mechanism; they do not quantify the common scheduler's
overhead versus an otherwise identical client without timestamps.

## Safety boundary and decision

No successful update may be acknowledged or published before synchronization.
This study does not relax that contract, disable conflict detection, or replace
recovery. Existing file formats and public APIs remain unchanged. A new scheduling
policy is justified only by repeatable evidence; otherwise keep the existing
defaults and report uncertainty. Process-crash tests are not physical power-loss
tests. No PALV library or Z: data is used.
