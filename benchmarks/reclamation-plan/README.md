# Bounded two-phase reclamation experiment

The acceptance comparison uses lighter instrumentation. Detailed profiling is
retained to investigate mechanism, not to select a favorable result. The
prototype fails this native gate; no longer service comparison is run.

## Versions and reproduction

Baseline `4d09365` has the same collector as production `4e0fc41`. Prototype
`6c6667b` adds bounded metadata capture, preparation outside the lock, and checked
application. Production is restored at `62af581`; the prototype remains in Git
history and is not enabled in main. The measured images were built before the
last two test-only additions; runtime source matches the prototype commit.

```sh
git worktree add --detach artifacts/plan-before 4d09365
git worktree add --detach artifacts/plan-prototype 6c6667b
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-cost-before-diag:local artifacts/plan-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-cost-before-light:local artifacts/plan-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-plan-diag:local artifacts/plan-prototype
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-plan-light:local artifacts/plan-prototype
python benchmarks/reclamation-cost/paired.py --out artifacts/plan-report/native --image-prefix catdb-reclaim-plan
python benchmarks/reclamation-cost/report.py artifacts/plan-report/native
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation-plan/plot.py artifacts/plan-report
```

Use fresh output paths; the runner refuses existing outputs. The plotting image
is the frozen client image at `1c22601`, built as described in the
[earlier reproduction guide](../reclamation-cost/README.md).

## Unchanged workload

Both sides enable background collection. Native workload: 100k x 256 B with two
readers and four writers (300 updates each); 4096 x 64 KiB with four writers (100
updates each); old-generation release after overwriting all 100k records, with
two readers and no ongoing writer. Three varied-order repeats for each scenario
and both instrumentation modes: 36 runs in total. Containers use 2 CPUs / 4 GiB.
All data is synthetic and disposable. No concurrent local heavy build/test or
other benchmark; hosted CI is separate.

P99 pools the two readers' raw samples per run and reports the median across
three runs. Worst response and worst worker lock hold are maxima across all
repeats. Other summary-table hold values use that table's own stated aggregation,
including main/request threads; they are not the worker-only chart metric.
Raw sorted samples are retained and validated against summaries.

Read windows end with reclamation, so duration and sample counts differ. Light
runs disable optional state-lock/worker profiling but retain request clocks,
allocation tracking and standard worker counters. They are not uninstrumented.
`work_ns` still measures timed state-lock phases; it does not measure the new
outside-lock preparation. The new `free_ns` also includes plan-buffer destruction.
Neither field is CPU time. Do not compare them as exact instruction costs.

## Safety and bounds

The prototype reserves preparation buffers before allocation and falls back to
the established bounded collector if the reservation fails. At most one plan per
worker captures 64 keys / 192 metadata entries, with up to 129 prefix entries per
key. Prepared reclamation remains limited to 128 versions / about 1 MiB, with
one oversized value allowed for progress. Maximum key lengths are conservatively
included in the reservation. No plan buffer is allocated during idle polling.

The cursor, target generation and captured first version must still match at
application; persistence must not be in progress. New versions may append, but
an intervening collector invalidates a stale plan. Values are charged as retained
until destruction outside the data lock. Buffers are released on discard/stop.

Seven additional tests cover appended/deleted values and pinned generations,
stale plans / persistence exclusion, 63/64/65 keys, 127/128/129 obsolete versions,
byte limits and oversized values, reservation below/at/above the limit, empty
and tombstone completion, and background fallback under buffer pressure. Model
phase tests use controlled state transitions; the existing integration suite
covers actual commits, checkpoints, old contents and process-crash recovery.

Heap is live requested Rust allocations including fixed sample vectors, measured
after worker stop and explicit drain before JSON construction. RSS, allocator
reserves and thread stacks are not measured. Resource budgets remain modeled
charges, not hard process-memory bounds. Existing data, PALV, stored formats and
public APIs are untouched.
