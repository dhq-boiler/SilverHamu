# Single-writer isolation experiment

This measures current catdb without changing engine code or defaults. All variants
use the same precomputed 8-byte key /256-byte value update schedule. The primary
conditions are C ABI group on/off, Rust group on/off, and SQLite WAL/FULL; separate
Rust lock-timing builds measure observation overhead and per-request phases.

100k rows, 8000 replacements, one writer, 2 CPU /4GiB, three runs with reordered
conditions, fresh Docker volumes. Each returned commit remains synchronous.
Primary timing has no retained reader, matching the earlier product workload.
Old-snapshot safety is tested after timing: retain the latest snapshot, commit an
additional value, verify every old value, release, verify latest and reopen.
SQLite-pinned is a separate sensitivity case that retains the initial snapshot
throughout timing; it is not pooled into the primary results.

## Reproduce

```sh
docker build -f benchmarks/Dockerfile -t catdb-peer-20261107:local .
docker build -f benchmarks/solo-update/Dockerfile -t catdb-solo:local .
python benchmarks/solo-update/run.py --out artifacts/solo-smoke-new --rows 1000 --ops 100 --reps 1
python benchmarks/solo-update/report.py artifacts/solo-smoke-new --smoke
python benchmarks/solo-update/test_report.py
python benchmarks/solo-update/run.py --out artifacts/solo-main-new
python benchmarks/solo-update/report.py artifacts/solo-main-new
```

Build and test before timed runs, not concurrently. Output folders must be new.
The runner owns only uniquely named disposable volumes/containers, uses no network,
and removes those resources after each run. A failed observation stops the run.
Report dependencies are Python and matplotlib (already in the harness image).

## Timing and limits

The per-operation timer includes begin, value construction, put, and commit.
Whole-loop throughput also includes saving the bounded timing/diagnostic records.
Rust diagnostic builds add clocks and fixed engine trace arrays plus a bounded
8000-record benchmark collector. Diagnostic throughput is not a primary result.
Trace phases telescope to the same request duration; tail phases are means for
the actual slowest1% in each run, not sums of unrelated component p99 values.

Collection is the explicit group gathering interval. commit_front includes other
coordination, locks, validation, encoding and reservation before the write.
sync is elapsed time around sync_all, including OS scheduling; it is not physical
disk service time. publish and finish include post-sync work/destruction/return.
clock_edges account for the outer sample timer versus the diagnostic timestamps.

Rust counts actual engine sync attempts and requires exactly one per timed commit.
C ABI and SQLite sync calls are not instrumented: their JSON count is null, not
zero. SQLite uses the existing comparison adapter with WAL and synchronous=FULL;
normal automatic checkpoints remain enabled. Reopen is normal in-process close/
open, not a new forced-power-loss test. The existing engine safety suite remains
separate from this benchmark.

Integrity: empty initial/missing key, all initial/final values, full old snapshot,
reopen, fixed operation count, bounded diagnostics, ordering and exact phase sums.
All C ABI/SQLite full-content hashes must agree per repetition; Rust verifies each
value against the same plan. No source hash mismatch or unfavorable run is waived.
