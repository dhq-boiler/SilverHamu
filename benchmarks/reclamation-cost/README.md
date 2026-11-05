# Reclamation completion-time investigation

The target is the existing **background-enabled** policy, not cooperative GC.
Both sides of every before/after comparison enable background reclamation.
The previous experiment's 319 ms result is historical; paired controls are rerun.

## Runtime revisions and decision

| Runtime | Commit | Key budget | Contention pause | Persistence pause |
|---|---|---:|---:|---:|
| Previous policy with diagnostic counters | 4d09365 | 64 | 1 ms | 1 ms |
| Fast prototype | bebff39 | 64 | 100 us | 1 ms |
| Small prototype | e8a1aa3 | 32 | 100 us | 1 ms |
| Tiny prototype | f8e3116 | 16 | 50 us | 1 ms |
| Retained policy, diagnostics only | 944bbf2 | 64 | 1 ms | 1 ms |

All retain 128-version / approximately 1-MiB portions, with one oversized value
allowed for progress. Cooperative/manual collection stays at 64 keys. Scheduling
prototypes are **not adopted** because they do not satisfy the combined p99,
maximum-wait and total-time goal. Published prototype commits reproduce the
experiments; current main restores the prior background policy and remains
opt-in. No real library or PALV deployment was changed.

## Native diagnosis and paired comparisons

Use the same `reclamation-bench` shapes and preallocated samples described in the
[previous methodology](../reclamation/README.md). Two readers, four writers for
mixed/large cases, 100k x 256 B or 4096 x 64 KiB, 2 CPUs / 4 GiB, three repeats.
The release case has no concurrent writer after the old snapshot is released.
All latest contents are checked before and after reopen; old values are checked
in targeted tests and sampled by the timing probe. No operation failure is retried.

Create fresh worktrees at the runtime revisions above. Build each with the report
checkout's Dockerfile, once with `service,lock-timing` (default) and once with
`service`. Use tags `catdb-reclaim-cost-before-diag:local` and
`catdb-reclaim-cost-before-light:local` for the baseline; analogous `fast`, `small`
and `tiny` prefixes for prototypes. Example:

```sh
git worktree add --detach artifacts/cost-before 4d09365
git worktree add --detach artifacts/cost-fast bebff39
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-cost-before-diag:local artifacts/cost-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-cost-before-light:local artifacts/cost-before
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" -t catdb-reclaim-cost-fast-diag:local artifacts/cost-fast
docker build -f "$PWD/benchmarks/reclamation/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-reclaim-cost-fast-light:local artifacts/cost-fast
python benchmarks/reclamation-cost/auxiliary.py --background-only --scenarios gc --out artifacts/cost-rerun/diagnosis
python benchmarks/reclamation-cost/paired.py --image-prefix catdb-reclaim-cost-fast --scenarios gc --out artifacts/cost-rerun/fast-pilot
python benchmarks/reclamation-cost/paired.py --image-prefix catdb-reclaim-cost-fast --out artifacts/cost-rerun/fast-full
python benchmarks/reclamation-cost/report.py artifacts/cost-rerun/fast-full
```

Repeat the paired runner for `small` with `--scenarios gc`, and `tiny` with all
three scenarios, matching the published directories. Alternate variant order in
round 2. The fast full-workload comparison is an additional confirmation after
the pilot; both datasets remain published and are not pooled selectively.

Detailed worker counters compile only under `lock-timing`. `work_ns`, `free_ns`,
`yield_ns`, actual condition-variable pause time and worker lifetime are wall
clock durations, **not CPU time**. They include any descheduling within each
interval. Requested sleep differs from elapsed sleep; their difference does not
uniquely identify scheduler delay, timer granularity or mutex reacquisition.
Lock misses and persistence deferrals distinguish the old shared 1-ms backoff.
Control-loop residual is not presented as CPU cost. The validator checks that
non-overlapping measured intervals do not exceed the worker lifetime.

Timing-omitted builds return zeros for the detailed profile (unknown, not proof
of no waits). They still count allocations, time requests and maintain standard
worker status counters. Paired diagnostic/light runs estimate observer effects;
neither native build is completely uninstrumented. Reader sample counts depend
on phase duration; p99 is calculated over each complete observed phase. Faster
collection changes the fraction of reads overlapping maintenance. We retain
that existing timing boundary and do not pad the improved run with extra cheap
reads to force a percentile win. No fixed-rate arrival or fixed-window result
is claimed.

Heap measures live requested Rust allocation sizes, including fixed sample
vectors. Post-drain values are recorded after stopping the worker and explicit
cleanup. RSS, allocator reserves and thread stacks are not measured. Resource
budgets are modeled charges; all final retained values are validated against
fixture contents. Native budget peaks include preparation. Actual image IDs are
recorded per run; rebuilt base images/compilers can differ.

## Main service confirmation of the fastest prototype

Both servers enable background reclamation. The before server uses the baseline
light image; the candidate uses the fast light image. Both disable optional
state-lock/worker profiling. The same single-request client, 100k x 256 B,
1/4 connections, 8k operations/case, three alternating repeats, full-content and
restart checks are retained. Append WAL, synchronous durable acknowledgement,
no timed checkpoint. Use the frozen original client runtime at 1c22601:

```sh
git worktree add --detach artifacts/cost-client 1c22601
docker build -f artifacts/cost-client/benchmarks/Dockerfile -t catdb-comparison:local artifacts/cost-client
python benchmarks/reclamation-cost/run.py --variant before=catdb-reclaim-cost-before-light:local --variant background=catdb-reclaim-cost-fast-light:local --single-variants before background --out artifacts/cost-rerun/service
python benchmarks/service-update/validate.py artifacts/cost-rerun/service
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/reclamation-cost/plot.py artifacts/cost-rerun/service
```

Unlike the previous runner, this one sets the background environment variable
for **both** named variants. Native and service runs never overlap heavy local
builds/tests or one another. All data is synthetic in disposable containers and
unique volumes. These short Docker/Linux experiments do not establish a global
optimal policy, native Windows/mobile performance or power-loss certification.
