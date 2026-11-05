# Request identity pilot

This is a kernel-version-specific feasibility observer, not a DB optimization. It follows opaque request slots plus dispatch sequence numbers, including update-return state. Worker-to-journal parentage is still unresolved, so long performance runs did not pass the prerequisite.

Use the existing `catdb-tail-profile:local` image (`service,backlog-batch,benchmark-metrics,sync-stall,lock-timing`) and comparison base image:

```powershell
python benchmarks/request-id/run.py --variant candidate-profile-off=catdb-tail-profile:local --variant candidate-profile-trace=catdb-tail-profile:local --out C:/scratch/request-pilot --reps 1 --ops 1000 --cases 1:200 4:800 --scenarios normal
python benchmarks/request-id/analyze.py C:/scratch/request-pilot
python benchmarks/request-id/test_analysis.py
python benchmarks/request-id/check_export_failure.py C:/scratch/request-export-failure
```

Output directories must be new. Data use disposable Docker volumes. The collector requires SYS_ADMIN and tracefs dynamic-probe support, and owns only its unique probe group and instance. It refuses preexisting colliding probe names because this kernel's miss statistics omit group names. Do not run other catdb services/probe collectors during capture. The ring is bounded to 16 MiB per CPU, capture to 180 seconds; dump and alias replacement occur after tracing stops. A forced kill may leave owned resources: inspect the instance/registered events and remove only those owned by this capture.

The analyzer rejects loss or incomplete entry/return pairing. A slot is a memory address alias, not an allocation-generation ID; dispatch counters prevent merging repeated dispatches. `more=0` means no data remain at that update, not that a journal or DB commit is durable. Final release, retries, worker ownership and logical request lifetime are separate concepts. Context from the current journal thread is retained without assigning worker requests by time proximity. Partial/requeue paths have synthetic tests; this pilot did not exercise them live.
