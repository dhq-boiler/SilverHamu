# Inline-key default qualification

Pre-agreed decision: short-key median RSS must fall at least 5%; mixed-load read
and update median trial p99 must worsen no more than 5% in either key shape.
No extra refusals/OOM, safety/compatibility failures, or repeated substantial tail
regressions are acceptable. An inconclusive result leaves defaults unchanged.
The 5% latency tolerance is a decision threshold, not statistical significance.

Use the current default inline-latest with Vec keys versus the same source with
inline-key. Both use the unchanged durable append WAL. Conditions: 1M aged rows,
256-byte values, 1 CPU /512MiB/no swap, 1000 scheduled arrivals/s, 8000 read-only
then 8000 mixed requests (80% reads, one reader/four writer workers). Keep an old
reader during each workload. Six runs per shape alternate baseline/inline,
inline/baseline, baseline/inline. Short keys are 8 bytes; mixed keys alternate
8/32 bytes, preserving numeric order via an 8-byte prefix. Values do not grow.

EQ/ST: existing Rust key boundary, order, update/delete/reinsert, snapshot and
point/range conflict tests plus every-run full verification and pinned reads.
BV/ER: existing 15-byte boundary/arbitrary-byte tests, staged-budget rejection
and recovery, real synchronization, save failures and process-kill recovery.
Compatibility: Rust/C ABI/service suites and tests/key_storage_transition.py,
which exchanges mixed-key WAL/checkpoint files between both implementations.
Fixture tests ensure half of one million keys are long and values remain 256B.

Build the comparison runtime prerequisite as described in benchmarks/README.md
(`catdb-comparison:local`), then from the repository root:

```sh
docker build -f benchmarks/inline-qualification/Dockerfile -t catdb-inline-qualification:baseline .
docker build -f benchmarks/inline-qualification/Dockerfile --build-arg EXTRA_FEATURES=,inline-key -t catdb-inline-qualification:inline .
python benchmarks/inline-qualification/test_fixture.py
python benchmarks/inline-qualification/run.py --out NEW/short/raw --shape short
python benchmarks/inline-qualification/run.py --out NEW/mixed/raw --shape mixed
python benchmarks/inline-qualification/report.py NEW
```

The safety Dockerfile uses the previously built catdb-capacity-gate Rust test
image as a compiler cache; it overwrites Cargo files, src and benchmarks with
the current checkout and reruns the suites. Build that prerequisite with
benchmarks/capacity-gate/Dockerfile, then build Safety.Dockerfile here.
Safety runs and builds must finish before performance measurements.

Each output directory must be new. Failed conditions are recorded and stop the
driver; do not repeat a failed condition to select a better sample. Seed setup
uses 2 CPU/4GiB outside the measured cap. All data is synthetic in owned Docker
volumes. Reopening is warm, readiness is polled every 100ms, and process kills
do not certify physical power-loss durability. RSS is server-only sampled peak;
cgroup usage and file cache are distinct. Fixed-rate throughput is not capacity.
No OS tracing or allocation diagnostics are enabled in measured servers.

The comparison and safety recipes use measured-Cargo.toml to preserve the original
pre-adoption default feature set. Default.Dockerfile instead tests the current
default, compares its service/library bytes to the measured candidate, and tests
rollback with --no-default-features --features service,inline-latest.
