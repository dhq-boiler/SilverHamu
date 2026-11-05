# Snapshot registration / reclamation comparison

This follow-up keeps the prior benchmark semantics, synthetic data and timing
windows. Both sides enable background reclamation; neither side uses cooperative
GC in the timed phase. No scheduling constants or durability behavior change.

## Runtime versions

- Baseline: `4d09365` (same production policy as `23c22b6`).
- Separate registry mutex: `0ae18bf`.
- Atomic shared-generation counts: `44207da`.

The first prototype releases a snapshot under a small registry mutex instead of
the data-state mutex. The second lets non-last releases decrement an atomic
counter; the last release removes the entry under the registry mutex after
checking for concurrent re-pinning. Registration and collection-floor selection
still happen under the state mutex. Neither prototype is a lock-free read path.
The benchmark captures **data-state** lock acquisitions; it does not count every
mutex or atomic operation in the engine.

Both candidates were rejected and production was restored at `0755213`.

## Reproduction

Use unique worktree/output paths; the runners reject existing output directories.
Build each version with the existing benchmark Dockerfile and explicit feature
sets. Baseline images are the same frozen images used in the previous report;
image IDs and container CPU/memory constraints are recorded per cohort.

```sh
git worktree add --detach artifacts/pins-before 4d09365
git worktree add --detach artifacts/pins-registry 0ae18bf
git worktree add --detach artifacts/pins-atomic 44207da
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-cost-before-diag:local artifacts/pins-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-cost-before-light:local artifacts/pins-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-pin-diag:local artifacts/pins-registry
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-pin-light:local artifacts/pins-registry
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-pin-atomic-diag:local artifacts/pins-atomic
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-pin-atomic-light:local artifacts/pins-atomic
python benchmarks/reclamation-cost/paired.py --out artifacts/pins-report/native --image-prefix catdb-reclaim-pin
python benchmarks/reclamation-cost/paired.py --out artifacts/pins-report/atomic --image-prefix catdb-reclaim-pin-atomic
python benchmarks/reclamation-cost/report.py artifacts/pins-report/native
python benchmarks/reclamation-cost/report.py artifacts/pins-report/atomic
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation-pins/plot.py artifacts/pins-report
python benchmarks/reclamation-cost/run.py --variant before=catdb-reclaim-cost-before-light:local --variant background=catdb-reclaim-pin-atomic-light:local --single-variants before background --clients 1 4 --reps 3 --out artifacts/pins-report/service
python benchmarks/service-update/validate.py artifacts/pins-report/service
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation-cost/plot.py artifacts/pins-report/service "catdb service: snapshot registry prototype"
```

The frozen client image is built at `1c22601`; see the
[previous commands](../reclamation-cost/README.md). Both servers are
background-enabled in this runner. The original first-prototype CI passed its
runtime tests but rejected a nested `if` lint; the second commit fixes the lint.

## Conditions and limits

Native: 100k x 256 B, two readers and four writers (300 updates each), or
4096 x 64 KiB with four writers (100 updates each); the old-generation release
case first overwrites all 100k records and has two readers but no ongoing writer.
Three alternating repetitions of each scenario and each profiling mode. CPU
limit 2, memory limit 4 GiB per measured container. Native latest contents and
reopen contents are checked in full; old contents are sampled in the probe and
verified in full by targeted safety tests.

Service: same 100k x 256 B, 1/4 clients, 8k operations per case, three alternating
repetitions; TCP and Python overhead included. No simultaneous local heavy build,
test or second benchmark. CI runs separately on hosted runners.

Raw sorted latency samples are published and percentiles recomputed. P99 is
pooled across the two native readers within each run, then median across three
runs. Worst latency is the maximum across all three runs, not a median of maxima.
The read window ends with collection; different collection durations change
sample counts. Do not extend fast runs with cheap reads to improve the percentile.
Light mode disables optional lock/worker profiling but retains request clocks,
allocation tracking and standard worker counters. Results are not uninstrumented.

Memory is live requested Rust heap, including preallocated sample vectors, not
RSS, allocator-reserved pages or thread stacks. Final modeled resource charges
must match the fixture after complete drain. The new registry adds fixed heap
and the atomic variant adds per-live-generation counters; neither changes the
public budget into a hard process-memory bound. All data is disposable; no PALV
library, Z: data, storage format or public API is changed by these prototypes.

After all measurements complete, validate all raw data and data-lock counts:

```sh
python benchmarks/reclamation-pins/validate.py artifacts/pins-report
```
