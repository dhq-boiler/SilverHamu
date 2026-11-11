# Kernel attribution of log synchronization

This follows `backlog-tail` without changing the engine, commit policy or client arrival generator. It compares **the same instrumented server image** with an external kernel observer off/on. It is not a comparison against a non-instrumented production binary.

Build the existing benchmark base as described in `benchmarks/README.md`, then:

```powershell
docker build -f benchmarks/backlog-tail/Dockerfile --build-arg CATDB_FEATURES=service,backlog-batch,benchmark-metrics,sync-stall,lock-timing -t catdb-tail-profile:local .
python benchmarks/sync-os/run.py --variant candidate-profile-off=catdb-tail-profile:local --variant candidate-profile-trace=catdb-tail-profile:local --out C:/scratch/catdb-sync-os --reps 3 --cases 1:200 1:250 4:400 4:800 --scenarios normal
python benchmarks/sync-os/analyze.py C:/scratch/catdb-sync-os
python benchmarks/sync-os/test_analysis.py
```

The output directory must not already exist. The workload uses a unique disposable Docker volume and internal network. Never use a production database or run unrelated SilverHamu servers during collection: scheduler filters identify the `silverhamu-server` and `catdb-client` thread names. Synchronization counts must agree exactly or attribution fails.

The collector requires a Linux kernel with tracefs, fsync, scheduler and block tracepoints. Docker Desktop's Linux VM supplied these in this run. The collector alone gets `SYS_ADMIN`, a private tracefs mount and host PID view; the DB remains non-root with its existing resource limits. No `--privileged`, host sysctl, global trace instance or storage policy change is used. A unique trace instance is enabled only around each workload; data are exported after tracing stops. No data values, authentication tokens or file paths are captured by these events. Raw traces do contain guest process names, thread IDs, block addresses and kernel symbols.

Tracing has a 180-second deadline and 16 MiB per CPU ring capacity. It is not an unbounded flight recorder. The analyzer rejects reported buffer losses, incomplete fsync calls, missing wakeups and sync-count mismatches. Trace output has microsecond timestamp resolution. `mono` and the Linux client's `perf_counter_ns` share the guest monotonic clock; a measured boundary/counter mismatch must not be silently adjusted.

The categories are:

- Scheduled: time when the target is assigned to a guest CPU, including interrupt and VM scheduling effects.
- Runnable: a wakeup or preemption until the next switch-in.
- Blocked: switch-out with a non-runnable state until wakeup. I/O-path attribution additionally requires a recorded writeback / `io_schedule` / journal-commit wait stack. Other waits remain unclassified.

Journal-worker execution and runnable delay are not separated here. Block events on shared virtual storage cannot be uniquely assigned to a WAL merely because they overlap its fsync. No physical disk latency or historical outlier cause is inferred from that overlap.

Test perspectives: EQ/ST covers same-syscall partitions and wakeup/preemption transitions; BV covers workload-window filtering and bounded capture; ER covers missing wakeups, incomplete events and observer cleanup. The existing engine and protocol safety suite covers committed-data recovery, old snapshots and conflicts; this experiment does not modify their implementation.

The initial 4-MiB-per-CPU cohort lost events in five cases and is retained separately in the report. The final 16-MiB-per-CPU cohort is the source of the main figures. Do not pool them. An injected export failure was checked to remove the owned trace instance. A forced collector kill or VM failure can prevent its cleanup; inspect the instance name in its `ready` file and remove only that instance after stopping it.
