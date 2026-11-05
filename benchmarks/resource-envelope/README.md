# Container resource envelope

This experiment measures the **unchanged default catdb service** under Docker
CPU quotas and memory limits. It is not a comparison with other DBMS products,
a cold-cache test, or a claim that a container emulates a dedicated small PC.

## Reproduce

From the repository root, with Linux containers available:

```sh
docker build -f benchmarks/Dockerfile -t catdb-resource:local .
python benchmarks/resource-envelope/test_client.py
python benchmarks/resource-envelope/run.py --out artifacts/resource-full
python benchmarks/resource-envelope/supplement.py --out-root artifacts/resource-diagnostics
```

The default matrix is 100,000 / 1,000,000 rows, 256-byte values, CPU quotas of
1 / 2, memory limits of 512 / 1024 / 2048 MiB, and three repetitions in a seeded
shuffled order. Each successful configuration runs two 8,000-request workloads
at 1,000 scheduled requests/s, reversing workload order in the second repetition.

- `read`: four read connections, point lookups only.
- `mixed`: one reader and four writer connections; 80% point reads and 20%
  independent one-request durable updates. Four connections do not imply four
  simultaneously executing writes, especially at the moderate offered rate.
- Each workload holds an additional old read snapshot. Every successfully updated
  key is checked against its pre-workload value through that snapshot afterward.
- Two queues, each bounded at 64 requests, separate read and write admission.
  Queue-full requests are recorded, never retried. Errors after submission are
  recorded separately and make the case fail; they are not silently treated as
  uncommitted. Latency is scheduled-arrival-to-completion, including client queue
  delay. Reported percentiles describe successful requests; rejection counts
  must be read alongside them. Throughput includes draining the queues.
- The client has its own 2-CPU / 2-GiB cgroup. Server cgroups include the Python
  supervisor. This is client-observed service performance, not engine capacity.
- `--cpus` limits aggregate CPU time, not CPU affinity. `--memory-swap` equals
  `--memory`; `memory.swap.max == 0` and the applied CPU/memory limits are checked.
- A 100-ms observer records cgroup CPU throttling, memory events, total/peak
  memory, anonymous/file memory, swap, and server-process RSS. Its cost is part
  of the limit. RSS and category maxima are sampled, whereas `memory.peak` is a
  cgroup high-water mark. They are not interchangeable. Cgroup peaks include
  lifecycle and helper costs, not only the timed workload.
- Database seeds are created outside the measured limit and copied into fresh
  uniquely named volumes. Loading the existing database is measured; ingestion
  is not. Host cache is not flushed. Shared/copy-warmed page cache can be charged
  outside the server cgroup. This does not demonstrate a cold low-memory host.
- Launch-to-ready includes Docker/client orchestration. The separate process
  ready field is polled (100 ms, or 500 ms without periodic sampling), so it is
  not a precise sub-millisecond initialization measurement.
- Every successful case verifies all values initially, after each workload, and
  after killing only its owned server child with SIGKILL and reopening the same
  database under the same limit. This is process-crash recovery, not power loss.
- Failed startup is retained, including server logs, cgroup memory events and
  Docker exit state. Increasing memory silently to make a matrix case pass is
  prohibited. `--recover-memory` is a separately labelled diagnostic, preserving
  the original failure result and checking the same database at the rescue limit.

For the higher-arrival and observer-cost diagnostic, use `--rows 100000 --cpus 1`
`--memory 512 --rate 5000 --reps 1`, alternate `--sample 0` and `--sample 1` for
three repetitions, and repeat the sampled workload at `--cpus 2`. This uses the
same 8,000 requests; it is a short overload diagnostic, not a steady-state soak.
An isolated OOM/recovery diagnostic uses `--memory 32 --recover-memory 512`.

`report.py --input RESULT_DIRS... --out REPORT_DIR` validates request timestamps,
counts, queue outcomes and resource records before copying compressed raw data
and producing the chart. Neither production durability nor its resource defaults
are changed by these scripts. All Docker resources have unique names; cleanup
targets only resources created by this invocation.

## Test perspectives

The user approved EQ (content), BV (limits and admission), ST (old generations
and restarts), ER (memory failure and recovery), measurement overhead, and
publication of negative outcomes before implementation.

- `test_client.py`: mixed-to-read transitions, old-value checks, exact acknowledged
  values, finite-queue refusal/drain, corrupted content rejection, zero-capacity
  rejection, and empty/single-sample percentiles. Fake storage tests the workload
  oracle; it is not a substitute for real database recovery.
- `run.py`: actual applied limits, full-content checks, old snapshots, child
  process termination/reopen, and diagnostic failure recovery.
- `report.py`: uniqueness/completeness of request records, timestamp order,
  successes/rejections/errors accounting, and absence of swap use.

Docker semantics: https://docs.docker.com/engine/containers/resource_constraints/
