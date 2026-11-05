# Observer loss comparison

Modes: `full` (15 probes); `minimal` (12, omitting rq_end/rq_update/rq_update_return and duplicate block/ext4 events); `capacity` (15 with return maxactive=128); `standard` (no dynamic probes, existing SCSI/block/journal events only). Mode is selected by the variant name. The minimal mode retains the exact pointer, dispatch, retry, free, journal-parent and channel context used by storage-stage analysis. It does not retain the previous per-data-update diagnostic detail.

Use existing `catdb-tail-profile:local` and `catdb-comparison:local`, disposable synthetic volumes and fresh output paths:

```powershell
python benchmarks/observer-loss/test_analysis.py
python benchmarks/observer-loss/run.py --variant candidate-profile-trace-full=catdb-tail-profile:local --variant candidate-profile-trace-minimal=catdb-tail-profile:local --out C:/scratch/observer-main --reps 3 --ops 8000 --cases 1:200 4:800 --scenarios normal
python benchmarks/observer-loss/analyze.py C:/scratch/observer-main
```

Before long runs, use `--reps 1 --ops 1000` and include `candidate-profile-off`, `candidate-profile-trace-capacity` and `candidate-profile-trace-standard`, pointing to the same image. Standard-only recording cannot establish the same direct request/journal mapping and does not pass the causal gate. Zero misses in a short capture is not proof that misses are impossible.

The quality check requires the exact probe set and zero cumulative misses, zero ring loss, cleanup and complete stage correspondence. Before-capture counters distinguish setup activity but are NOT subtracted to waive misses. Return counters combine entry-probe and return-instance losses; an entry/return match does not prove a specific reentry mechanism. No global kernel setting or database durability setting changes.

The private instance, unique registry group, 16 MiB/CPU ring and 180-second deadline remain. A forced kill can still leave resources; cleanup must only remove owned resources. Export failure can be checked with `check_export_failure.py <new-path>`. Raw kernel pointers are anonymized before export. Host I/O/device latency remains outside the observer's direct visibility.
