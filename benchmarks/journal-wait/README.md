# Journal wait feasibility pilot

This observer adds journal phases and journal-thread scheduling to `sync-os`, and removes per-switch stack triggers. It deliberately does not match block events to journal transactions: the available output lacks stable request identity and journal association. Full-length performance measurement did not pass this prerequisite.

Use the same server image built by `benchmarks/backlog-tail/Dockerfile` with features `service,backlog-batch,benchmark-metrics,sync-stall,lock-timing` and the existing comparison base image. Reproduce only the short pilot:

```powershell
python benchmarks/journal-wait/run.py --variant candidate-profile-off=catdb-tail-profile:local --variant candidate-profile-trace=catdb-tail-profile:local --out C:/scratch/journal-pilot --reps 1 --ops 1000 --cases 1:200 4:800 --scenarios normal
python benchmarks/journal-wait/analyze.py C:/scratch/journal-pilot
python benchmarks/journal-wait/test_analysis.py
python benchmarks/journal-wait/check_export_failure.py C:/scratch/journal-export-failure
```

Output folders must be new. Synthetic data remain in disposable Docker volumes. Do not run unrelated SilverHamu services during tracing; scheduler filters use thread names. The collector alone uses SYS_ADMIN to mount tracefs and create a unique instance; no global tracing policy is changed. It is bounded to 180 seconds and 16 MiB per CPU. The analyzer rejects loss, incomplete phases, wrong transaction IDs and missing wakeups. Stack collection is checked absent. Guest scheduler timestamps and block completions do not measure the Windows host or physical device.

The journal partition is start-to-end on the same device, transaction and journal thread. Equal journal/fsync counts do not prove ownership. Blocked time stays unclassified as to underlying I/O versus worker/lock waits. No nearest-event matching is performed for block requests; partial completion and flush-sector differences make that unsafe here.

Test perspectives: complete/missing/wrong phases and IDs, nested transitions, blocked/runnable/preempted states, missing wakeup, workload start/end boundaries, and cleanup after export failure. Existing engine/protocol tests verify unchanged durability and API behavior. A collector killed without running cleanup can leave its instance; inspect its `ready` file and operate only on that instance.
