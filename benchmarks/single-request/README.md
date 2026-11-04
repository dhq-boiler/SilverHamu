# Single-request service API comparison

The operation generator, keys, values, timing boundaries, Python threads and
verification are unchanged from `benchmarks/runner.py`. Only the adapter used
for timed single-request cases changes. It makes begin/rollback local no-ops,
uses get_latest/scan_latest, and stages one value locally before put_commit.
Local hex conversion and staging remain inside the original operation timer.
Initialization and full-content verification always use the original API.

Three variants distinguish API improvements from old-path regressions:
- before: original service runtime at `92d8917` (same service as `cd6df02`);
- legacy: new server at `96769c4`, original three-request adapter;
- single: that same new server, single-request adapter.

100,000 records, 256-byte values, 8,000 operations per case, 1/4 clients;
lookup, 100-row keyset scan and durable single-record replacement. Each variant
runs three times, variant order reverses in round 2 and case order is shuffled
identically across variants. Each client/server has 2 CPUs and 4 GiB. Connections
are reused and warmed. The main run has diagnostics disabled, no scheduled
checkpoint and disjoint writer keys. This is a closed-loop warm-cache Python API
benchmark, not an engine ceiling or a low-memory workload. No failed operation
is retried. Lost mutation responses remain uncertain; the API adds no deduplication.

## Reproduce

Run at the report commit with Docker Desktop/Linux and host Python. Worktree paths
and output must be new. All fixtures are synthetic in unique named volumes.

```sh
git worktree add --detach artifacts/single-before 92d8917
git worktree add --detach artifacts/single-after 96769c4
git worktree add --detach artifacts/single-client 1c22601
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-sync-cohort:local artifacts/single-before
docker build -f "$PWD/benchmarks/fixed-log/Dockerfile" --build-arg CATDB_FEATURES=service -t catdb-single-request:local artifacts/single-after
docker build -f artifacts/single-client/benchmarks/Dockerfile -t catdb-comparison:local artifacts/single-client
python benchmarks/single-request/run.py --variant before=catdb-sync-cohort:local --variant legacy=catdb-single-request:local --variant single=catdb-single-request:local --single-variants single --clients 1 4 --reps 3 --out artifacts/single-rerun
python benchmarks/service-update/validate.py artifacts/single-rerun
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/single-request/plot.py artifacts/single-rerun
```

The original run reused the frozen before/client images recorded in environment.json.
Rebuilding can change compiler/dependency/base-image patch versions; image IDs in
that file identify the actual measured binaries. The candidate image used runtime
96769c4; incidental rustfmt-only changes in lib.rs were reverted before commit.
Source hashes captured at run start describe the working tree, not the baseline.

Each scan_latest is a new snapshot. It does not replace a multi-page transaction
that must retain one snapshot. The benchmark performs one 100-row scan per
transaction in every mode. Writes still require durable sync and publication
before success. The regular append WAL is used, not the fixed-log prototype.

Older PostgreSQL/Oracle data is retained, but peers are not rerun here. Do not infer
a new product ranking from comparisons across separate runs. All regressions and
min/max repeat variability remain in the plot and raw data.

## Auxiliary ping comparison

After the main run finishes, compare version-only pings on the same 100,000-row
fixture. This uses 1,000 requests per condition, 1/4 clients and three repeats.
The timing adapter performs one ping per operation in both modes; existing
full-content checks and restart checks remain outside the timed interval.

```sh
python benchmarks/single-request/ping_run.py --variant before=catdb-sync-cohort:local --variant after=catdb-single-request:local --clients 1 4 --workloads ping --ops 1000 --reps 3 --out artifacts/single-rerun/ping
python benchmarks/single-request/validate_ping.py artifacts/single-rerun/ping
python benchmarks/single-request/report.py artifacts/single-rerun
```

This auxiliary comparison does not measure the effect of background healthchecks
on mixed traffic. It isolates the cost of a ping with the same database size.
