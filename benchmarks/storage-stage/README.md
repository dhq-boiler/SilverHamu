# Virtual storage stage observer

This extends the flush-parent observer with `storvsc_queuecommand` entry/return, `scsi_done`, and `storvsc_on_channel_callback` entry/return. The running kernel BTF supplies `sizeof(struct request)`. SCSI command addresses are converted using the kernel PDU layout and then anonymized into the same request namespace. No guessed fixed offset or time-nearest pairing is used.

The three spans are block dispatch to driver entry, driver entry to SCSI completion notification, and notification to the member-bearing flush callback. Driver entry is **not the exact transport-send instant**, and SCSI completion is **not the host's exact completion instant**. The middle span includes driver work, transport, host/storage work, virtual CPU/interrupt delivery and receive processing. It is not physical disk service time. The channel callback establishes receive-processing context, not exclusive ownership of a callback by one request.

Use `catdb-tail-profile:local` (`service,backlog-batch,benchmark-metrics,sync-stall,lock-timing`) and `catdb-comparison:local` images, with synthetic disposable volumes and fresh output paths.

```powershell
python benchmarks/storage-stage/test_analysis.py
python benchmarks/storage-stage/run.py --variant candidate-profile-off=catdb-tail-profile:local --variant candidate-profile-trace=catdb-tail-profile:local --out C:/scratch/storage-pilot --reps 1 --ops 1000 --cases 1:200 4:800 --scenarios normal
python benchmarks/storage-stage/analyze.py C:/scratch/storage-pilot
```

Only after matching and zero-loss checks pass, use a new output directory with `--ops 8000 --reps 3`. Invalid captures remain published and are excluded from causal interpretation. No selective rerun is implied. `report.py` expects sibling pilot, validation and export-failure results from this study.

Fifteen owned probes, a private trace instance, 16 MiB/CPU ring and 180-second limit bound recording. SYS_ADMIN and the specific kernel symbols/BTF layout are required. Export and aliasing run after timing. Cleanup removes only owned probes/instances; a forced process/VM kill can bypass it. Use `check_export_failure.py <new-output>` after timing. Existing request/journal/flush tests remain applicable; synthetic events cover reuse, retries, failures, missing entries/returns, shared receive callbacks and malformed BTF. Physical power loss is not tested.
