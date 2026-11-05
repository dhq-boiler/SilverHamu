# Leading history-key reclamation comparison

This experiment changes the bounded collector loop, not snapshot ownership,
worker scheduling, durability or public APIs. It is evaluated first with the
same native benchmark, using light instrumentation as the acceptance gate.

## Reproduce the native comparison

Baseline `4d09365` has the same collector as production `a861a50`. Candidate
`c8d205f` uses `BTreeSet::pop_first` while no retained key precedes the cursor,
transferring ownership of the key rather than cloning/searching/removing it
separately. If a pinned prefix remains, the key is reinserted and collection
continues through the established range traversal. Partial collection restores
the popped key before returning. A final range pass and the generation floor
keep their existing semantics.

Fresh paths are required. Baseline images are frozen from the prior experiment;
image IDs are recorded with this cohort. Build the candidate twice:

```sh
git worktree add --detach artifacts/front-before 4d09365
git worktree add --detach artifacts/front-candidate c8d205f
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-cost-before-diag:local artifacts/front-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-cost-before-light:local artifacts/front-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-front-diag:local artifacts/front-candidate
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-front-light:local artifacts/front-candidate
python benchmarks/reclamation-cost/paired.py --out artifacts/front-report/native --image-prefix catdb-reclaim-front
python benchmarks/reclamation-cost/report.py artifacts/front-report/native
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation-plan/plot.py artifacts/front-report "Leading history-key reclamation comparison"
```

The plotting image is the frozen client image at `1c22601`; see the
[previous reproduction commands](../reclamation-cost/README.md).

## Workload, validation and limits

Both sides enable background reclamation. Three varied-order repeats, detailed
and light instrumentation, 2 CPUs / 4 GiB per container. Native mixed: 100k x
256 B, two readers and four writers (300 updates each). Large values: 4096 x
64 KiB, two readers and four writers (100 updates each). Release case: overwrite
all 100k records while holding an old generation, then release it with two readers
and no ongoing writer. No concurrent local heavy build/test or other measurement.
Hosted CI runs separately.

P99 pools both native reader sample sets within each run, then takes the median
of three repetitions. Worst response and worker lock hold are maxima over all
three repetitions. Raw samples are saved and percentiles recomputed. The read
window ends with collection; shorter runs can contain different sample counts
and proportions of contended reads. Light instrumentation disables optional lock
profiling but retains allocation tracking, request clocks and worker counters.

Latest full contents and reopen contents must match. Targeted tests retain full
old generations, plus the existing write/read/range-conflict, persistence-failure,
checkpoint and process-crash tests. Added tests ensure a pinned leading key does
not hide collectible suffixes across a 64-key boundary and that 63/64/65-key
batches resume without missing keys. Existing 128-version and byte-bound tests
also exercise restoration of a partially collected leading key. The maximum
64 keys, 128 obsolete versions and approximately 1 MiB per call remain unchanged.

Heap is live requested allocation size including fixed sample vectors, not RSS,
allocator reserves or stacks. After worker stop and explicit drain, both modeled
retained charges and requested heap are checked. No new persistent metadata or
preparation buffer is introduced. The minimum Rust version remains 1.89; the
implementation uses [`pop_first`](https://doc.rust-lang.org/std/collections/struct.BTreeSet.html#method.pop_first)
rather than requiring newer extraction APIs.

## One additional backoff condition

After the first cohort reduced read p99, maximum wait and timed collection work
but missed total completion, one follow-up condition was declared before running:
`a965e2d` combines the same leading-key loop with 500 us contention backoff.
Persistence deferral stays 1 ms; batch/idle pauses and safety guarantees are
unchanged. These cohorts are not pooled or selected by their best repetition.

Build this commit as `catdb-reclaim-front-half-diag:local` and
`catdb-reclaim-front-half-light:local` using the same two commands/features above.

```sh
python benchmarks/reclamation-cost/paired.py --out artifacts/front-report/half --image-prefix catdb-reclaim-front-half
python benchmarks/reclamation-cost/report.py artifacts/front-report/half
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation-plan/plot.py artifacts/front-report "Leading-key collection with 500 us contention backoff" half
```

## Decision

Neither cohort meets the combined light-mode acceptance targets. Production is
restored at `eea36b1`; candidates remain in history. No longer service comparison
is run after this failed native gate.
