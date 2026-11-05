# Background reclamation comparison

Three scheduling candidates were measured with a common native harness:

| Candidate | Commit | Contended lock | Pending work |
|---|---|---|---|
| Conservative (retained, opt-in) | 424e2a5 / 99b92b8 | pause 1 ms | up to 16 portions/yields, then pause 100 us |
| Burst (not retained) | afcef4c | bounded retry/yield burst | up to 16 portions, then pause 100 us |
| Paced (not retained) | 0c2009d | pause 10 us | pause 10 us after each portion |

All policies keep 64-key / 128-version / approximately 1-MiB portions, preserve
pinned snapshots, and free detached values outside the data lock. OS scheduling
can extend pauses. Service defaults remain cooperative, without a worker.

## Main API comparison

Same 100,000 records x 256 B, single-request API, 1/4 clients, three repetitions,
8,000 operations per condition, as the previous [read-GC comparison](../read-gc/README.md).
Before is 80423f2 (runtime c0c8cff); background uses the selected conservative
runtime at 424e2a5, restored in 99b92b8 with additional failure tests. Two CPUs and
4 GiB each for client/server; append WAL, synchronous durability, no scheduled
checkpoint. Order reverses in round 2; operation order shuffles identically.
No retries. Full contents and reopen are verified outside timing.

Use new paths from the report checkout:

```sh
git worktree add --detach artifacts/reclaim-before 80423f2
git worktree add --detach artifacts/reclaim-selected 99b92b8
git worktree add --detach artifacts/reclaim-client 1c22601
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-read-gc:local artifacts/reclaim-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclamation-light:local artifacts/reclaim-selected
docker build -f artifacts/reclaim-client/benchmarks/Dockerfile -t catdb-comparison:local artifacts/reclaim-client
python benchmarks/reclamation/run.py --variant before=catdb-read-gc:local --variant background=catdb-reclamation-light:local --single-variants before background --out artifacts/reclaim-rerun/service
python benchmarks/service-update/validate.py artifacts/reclaim-rerun/service
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation/plot.py artifacts/reclaim-rerun/service
```

The runner sets `CATDB_BACKGROUND_RECLAMATION=1` only for variant `background`.
The main API server build has lock-timing disabled. It does not run the native
probe or its allocator. All fixtures live in unique disposable Docker volumes.

## Native workloads and observer comparison

`reclamation-bench` uses the same workload shapes as `read-gc-bench`: two readers
with four writers (100k x 256 B or 4096 x 64 KiB), and release of an old snapshot
pinning 100k overwritten 256-B values. Each run verifies all latest values before
and after reopen. Targeted tests verify the complete pinned old snapshot; the
probe samples its key 0. Reads continue to phase completion, minimum 1,000 per
thread, at most 2 million; writers perform 300 or 100 updates each.

This revision preallocates 2 million latency slots per reader before timing,
keeps raw u64 samples until after the heap snapshot, and records post-drain heap
and engine charges. That fixes the earlier sample-growth/JSON-construction
confound. Fixed sample capacity (32 MB plus writer slots) is included in heap
figures for **both** modes. Heap is requested live Rust allocation bytes, not
RSS, reserved allocator memory, or thread stacks. Final memory is sampled after
joining readers, disabling/joining the worker and draining synchronously.

Within each candidate, cooperative/background use the **same binary**, with the
mode enabled only at the timing barrier after identical preparation. Thus the
native comparison isolates scheduling; the main comparison also includes the
small engine changes relative to the prior release. All run sequentially under
2 CPUs / 4 GiB. Three repeats reverse variant order in round 2.

Four series distinguish lock timing from behavior: cooperative/background with
`service,lock-timing`, and cooperative-light/background-light with `service`.
Both still have allocator accounting, latency clocks and the worker's own
counters; the light series is **not a completely uninstrumented engine ceiling**.
The comparison estimates lock-timer observer effects, not all instrumentation
costs. Drop's lock acquisition is absent from the inherited wait counter; worker
lock hold is recorded separately and included in reported maxima. The service
chart remains the uninstrumented API comparison.

```sh
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclamation-diag:local artifacts/reclaim-selected
python benchmarks/reclamation/auxiliary.py --out artifacts/reclaim-rerun/auxiliary
python benchmarks/reclamation/report.py artifacts/reclaim-rerun/auxiliary
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation/native_plot.py artifacts/reclaim-rerun
```

For the other candidates, create fresh worktrees at afcef4c and 0c2009d and build
both service,lock-timing and service images with prefixes `catdb-reclamation-burst`
and `catdb-reclamation-paced` (suffixes `-diag:local` / `-light:local`). Run the
same runner with `--image-prefix PREFIX` into separate new output directories.
Report at each directory with `report.py`. The report checkout has all runner
scripts; do not rely on scripts present in older runtime worktrees.

All three candidates and unfavorable repeats remain published. Candidate cohorts
ran sequentially, not randomized across policies; compare each to its paired
cooperative control and do not infer a statistically established global ranking.
Actual image IDs identify measured binaries; rebuilds may change toolchains or
base-image patches. Native peak budget counters include preparation; worker
counters start at the timed barrier. Real power loss and full-disk operations
are not part of these experiments.
