# Read-only snapshots, unlocked copies and bounded reclamation

The main comparison reuses the single-request client on **both** versions.
Before is 14e8a7d (runtime 96769c4); after is c0c8cff. It uses the ordinary
append WAL, features `service`, with timing instrumentation disabled. Each
condition has 100,000 records, 256-byte values, 8,000 operations, 1/4 clients,
and three repetitions. Case order and verification are unchanged from the
[single-request methodology](../single-request/README.md). No peer DBMS is rerun.

Run from the report checkout with Docker and Python. Paths must not exist:

```sh
git worktree add --detach artifacts/read-before 14e8a7d
git worktree add --detach artifacts/read-after c0c8cff
git worktree add --detach artifacts/read-client 1c22601
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-single-request:local artifacts/read-before
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-read-gc:local artifacts/read-after
docker build -f artifacts/read-client/benchmarks/Dockerfile -t catdb-comparison:local artifacts/read-client
python benchmarks/single-request/run.py --variant before=catdb-single-request:local --variant after=catdb-read-gc:local --single-variants before after --out artifacts/read-rerun
python benchmarks/service-update/validate.py artifacts/read-rerun
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/read-gc/plot.py artifacts/read-rerun
```

## Native diagnostic comparison

Separate from the main API chart, `read-gc-bench` uses two native reader threads:

- `mixed`: 100,000 x 256 B, four writers x 300 durable replacements, point reads.
- `large`: 4,096 x 64 KiB, four writers x 100 replacements, four-row page reads.
- `gc`: 100,000 x 256 B, all records replaced while an old snapshot is pinned;
  time its release and **all remaining reclamation**, with two concurrent readers.

Writer keys are disjoint. Each run creates a new fixture, retains the old
snapshot through writes, verifies every latest value, reopens, and verifies
all values again. Old-value preservation is also checked in targeted tests;
the native timing probe samples key 0 of the pinned snapshot, not its full scan.
Readers run until the phase ends, with at least 1,000 requests per thread.
There are no retry or rejection injections; errors fail the run.

Both native builds have identical state-lock timing instrumentation and an
allocation-counting global allocator. They **are not uninstrumented throughput
measurements**. Heap bytes are requested live Rust allocation sizes, not RSS,
allocator capacity or a hard memory bound. Samples and result construction also
allocate. Budget figures are the engine's separate modeled resource charges.
Lock hold includes transaction Drop, but the older lock-wait counter omits
Drop's acquisition wait. OS scheduling can inflate measured lock hold.

The baseline patch adds only those diagnostics and API shims (`begin_read`
delegates to `begin`; explicit GC is a no-op because old Drop drains it).
It does not change baseline copies or GC. Apply to a **new** baseline worktree:

```sh
git worktree add --detach artifacts/read-before-diag 14e8a7d
git -C artifacts/read-before-diag apply ../../benchmarks/read-gc/baseline-diagnostics.patch
cp src/bin/read-gc-bench.rs artifacts/read-before-diag/src/bin/read-gc-bench.rs
docker build -f "$PWD/benchmarks/read-gc/Dockerfile" -t catdb-read-gc-before-diag:local artifacts/read-before-diag
docker build -f "$PWD/benchmarks/read-gc/Dockerfile" -t catdb-read-gc-after-diag:local artifacts/read-after
python benchmarks/read-gc/auxiliary.py --out artifacts/read-rerun/auxiliary
python benchmarks/read-gc/auxiliary_report.py artifacts/read-rerun/auxiliary
```

Each container has 2 CPUs / 4 GiB. Conditions run sequentially, three times,
with variant order reversed in round 2. No heavy local builds or tests overlap
measurement. Environment files retain actual image IDs; rebuilding can change
compiler and base-image patch versions. All fixtures are synthetic and disposable.
The service image was built before a documentation-only Rust comment change;
the diagnostic probe was subsequently rustfmt-formatted without behavior changes.
These experiments do not simulate real power loss or a disk-full filesystem.
