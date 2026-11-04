# Log synchronization investigation

This follow-up keeps the original `benchmarks/runner.py` operations unchanged.
It tests durable single-row updates over TCP, including begin/put/commit and
Python driver overhead, with 100,000 records and 256-byte values. Every condition
has 8,000 attempts. Each client and server is limited to 2 CPUs and 4 GiB.
Fresh Docker named volumes and private networks isolate all synthetic data.

## What is being compared

- **Current catdb:** service with group handoff, equivalent to `dfebd0a`.
- **Candidate:** handoff plus bounded re-formation of a recently full group.
- **PostgreSQL and Oracle Free:** exact image digests from the original comparison,
  rerun contemporaneously. Serializable client settings and synchronous durable
  commit settings remain the same. This does not assert equivalence of every
  engine's isolation features.

The candidate waits up to 1 ms for four requests only when it has actually waited
behind a prior group and its old collection budget has expired, or when at least
four successful commits finished within the preceding 2 ms. Otherwise the
existing 200 us budget applies. Both waits finish early when four requests are
present. The budgets do not accumulate: at most one of the 1 ms waits applies.
A lone client from an idle/small-group state receives no additional wait. A sudden
transition from a full group to a lone request within 2 ms can incur one bounded
extra wait, after which the small-group result resets the hint. Operating-system
scheduling can exceed requested wait durations; this is not a real-time bound.
The hint has one fixed-size entry per database. No queue bounds, file formats,
conflict checks, synchronization calls or success-acknowledgement ordering change.

## Reproduce

Docker Desktop/Linux and Python are required. Run from the repository root.
Use a fresh output directory; existing outputs are never replaced. The baseline
worktree must not be the instrumented baseline of an older experiment.

```sh
git worktree add --detach artifacts/log-sync-baseline dfebd0a
docker build -f artifacts/log-sync-baseline/benchmarks/service-update/Dockerfile -t catdb-update-handoff:local artifacts/log-sync-baseline
git worktree add --detach artifacts/log-sync-candidate 1c046f2ede2e979b1e228480ed10b55cd61a7f83
docker build -f artifacts/log-sync-candidate/benchmarks/service-update/Dockerfile --build-arg CATDB_FEATURES=service,handoff-refill -t catdb-sync-cohort:local artifacts/log-sync-candidate
# Preserve/rebuild the original client independently from the server images.
git worktree add --detach artifacts/log-sync-client 1c22601
docker build -f artifacts/log-sync-client/benchmarks/Dockerfile -t catdb-comparison:local artifacts/log-sync-client
docker pull postgres@sha256:3a4264520e121176619f54ff867d12774bc81e95e7b84db58d63bccdb6144b81
docker pull container-registry.oracle.com/database/free@sha256:cf540c3fa190d7cffad08c491652ac07fc70314e510f6b87890449517d565e94
python benchmarks/log-sync/run.py --out artifacts/log-sync-rerun --reps 3
python benchmarks/log-sync/validate.py artifacts/log-sync-rerun
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/log-sync/plot.py artifacts/log-sync-rerun
```

The full run measures 1 and 4 connections for each path three times. Variant
order reverses in repeat 2 and rotates in repeat 3; client-count order is shuffled
with the same seed per repeat. Initialization, warm-up, all-value verification
and restart are excluded from timing. All values and empty/end boundaries are
checked after each case and after restart; corresponding hashes must agree
across products. Operation errors are retained and stop the run. No automatic
commit retries are allowed. There is no scheduled concurrent checkpoint, mixed
read workload, cold-cache claim or engine-only ceiling claim.

## I/O probes

`probe.py` compares split/combined writes, fsync/fdatasync, physical preallocation
with unchanged EOF, and fixed-size overwrite. `probe_sync_write.py` compares
fsync, fdatasync, O_SYNC, O_DSYNC and RWF_DSYNC. Each uses a 32 MiB seeded file,
512 batches of 600 bytes, three shuffled repeats, and full-file hash verification.
These are Linux I/O probes, not 100,000-record DBMS measurements. Fixed-size
results are a diagnostic only; no new WAL format is adopted.

Run either script in the original client container with an empty, disposable
named volume mounted at `/data`, and this directory mounted read-only at `/probe`.
Use `--cpus 2 --memory 4g`. The scripts refuse to reuse `/data/probe`. The complete
JSON is printed to stdout. Only the probe's own files are removed.

## Diagnostic overhead

Build current and candidate images with `lock-timing`, then use the existing
paired runner with only the diagnostic variants selected for profiling:

```sh
docker build -f artifacts/log-sync-baseline/benchmarks/service-update/Dockerfile --build-arg CATDB_FEATURES=service,lock-timing -t catdb-update-handoff-diag:local artifacts/log-sync-baseline
docker build -f artifacts/log-sync-candidate/benchmarks/service-update/Dockerfile --build-arg CATDB_FEATURES=service,handoff-refill,lock-timing -t catdb-sync-cohort-diag:local artifacts/log-sync-candidate
python benchmarks/service-update/run.py --variant current=catdb-update-handoff:local --variant current-diag=catdb-update-handoff-diag:local --variant candidate=catdb-sync-cohort:local --variant candidate-diag=catdb-sync-cohort-diag:local --profile-variants current-diag candidate-diag --clients 4 --workloads update --reps 3 --out artifacts/log-sync-rerun/observer
python benchmarks/service-update/validate.py artifacts/log-sync-rerun/observer
python benchmarks/service-update/analyze.py artifacts/log-sync-rerun/observer
python benchmarks/log-sync/report.py artifacts/log-sync-rerun
```

Timing includes server instrumentation, diagnostic response encoding and client
collection. It can alter arrival timing/group formation; a positive throughput
change with instrumentation is not negative overhead. Main comparison results
have instrumentation disabled. Trace stage definitions are in the
[preceding follow-up](../service-update/README.md#diagnose-stages-and-measure-observer-overhead).
