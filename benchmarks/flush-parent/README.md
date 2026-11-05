# Shared-flush parentage observer

Version-specific Linux tracing, not a DB optimization. Ten owned probes follow request dispatch/data completion and parent sequence callbacks inside flush completion. A callback without members can be a reference-release call, not a completed flush. Members are lists, not a single-parent assumption. Guest timings do not measure physical host/device service time.

Use existing `catdb-tail-profile:local` and `catdb-comparison:local` images. The server features are `service,backlog-batch,benchmark-metrics,sync-stall,lock-timing`. Output paths must be new; all DB data use disposable volumes.

```powershell
python benchmarks/flush-parent/test_analysis.py
python benchmarks/request-id/test_analysis.py
python benchmarks/flush-parent/run.py --variant candidate-profile-off=catdb-tail-profile:local --variant candidate-profile-trace=catdb-tail-profile:local --out C:/scratch/flush-pilot --reps 1 --ops 1000 --cases 1:200 4:800 --scenarios normal
python benchmarks/flush-parent/analyze.py C:/scratch/flush-pilot
```

Only after parentage, loss checks and observer behavior are validated, repeat with a new output path, `--reps 3 --ops 8000`. A false gate requires examining unresolved relationships, not assigning ownership by temporal adjacency. Run `check_export_failure.py` with a new scratch directory after timing. Run `report.py <main-output>` after validation and provenance collection; report generation expects the sibling pilot, validation and export-failure artifacts used in this study.

The collector requires SYS_ADMIN and tracefs, uses a private instance and unique event group, never truncates the shared registry, rejects conflicting event names, and cleans up its own resources. The ring is bounded to 16 MiB/CPU and capture to 180 seconds. No concurrent probe collector should run. Forced termination may bypass cleanup; remove only owned resources. Request and queue pointers are aliased before export. Miss counters and entry/return matching must pass before results are interpreted.

Synthetic tests cover multiple parents, context handoff, reuse/free, unknown/preexisting parents, CPU isolation, duplicates, missing returns, errors, data-versus-flush phases and reference-release callbacks. Existing request tests cover partial completion/requeue. Real hardware error/power-loss coverage is not claimed.
