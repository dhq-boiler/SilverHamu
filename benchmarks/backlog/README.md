# Bounded backlog draining under controlled sync stalls

This experiment changes how **already queued independent updates** reach the
service. It does not acknowledge writes early, replace `sync_all`, merge updates
to the same key, or turn a set of independent transactions into one transaction.
The production-default service remains unchanged; `backlog-batch` is opt-in.

## Candidate and comparison contract

The baseline sends one `put_commit` and waits for its reply on each connection.
The candidate also does that when no other work is ready. When a worker finds
multiple queued updates, it drains up to **eight distinct keys**, immediately,
and sends one `put_batch_commit`. It never waits for more arrivals. A repeated
key stops the current group; the next request retains its place at the head of
the worker's work. This avoids losing or reordering successive same-key writes.

The service prepares a separate transaction for every entry and uses the existing
bounded multi-transaction commit path. Each transaction retains its own conflict
check, version and outcome. Group synchronization remains real and precedes
publication/acknowledgment. The new feature exposes the batch primitive without
enabling the older `commit-window` feature's additional collection delay.

Both policies have the same **64 outstanding-request credits**, divided equally
among their one or four workers. Credits include FIFO entries, any duplicate-key
carry, and the active batch, until a reply is received. A full worker budget
rejects admission; there is no retry or reassignment. This differs from the prior
arrival-rate study's 64 pending slots plus active requests, so its numeric results
are historical, not directly interchangeable. A batch does allow more simultaneous
logical transactions per connection: that is the intended mechanism, not equal
transaction concurrency. Network round trips, parsing and synchronization can all
be amortized; an improvement is not attributed to disk work alone.

## Opt-in API

Build the experimental service with `--features service,backlog-batch` to expose:

```json
{"op":"put_batch_commit","entries":[{"key":"01","value":"aa"},{"key":"02","value":"bb"}]}
```

Example response:

```json
{"ok":true,"results":[{"ok":true,"version":"1"},{"ok":true,"version":"2"}],"reopen_required":false}
```

`results` is ordered like the input. Top-level `ok` means the batch request was
processed; **each `results[i].ok` is the independent commit result**. A conflict
in one entry does not roll back another successful independent transaction.
Callers must inspect every item. This is not an all-or-nothing batch API. Fatal
uncertain/poisoned outcomes set `reopen_required` and close the connection after
the response. A lost response leaves the corresponding commit outcome uncertain;
do not infer rollback or blindly retry from a disconnected socket.

Limits: 1–8 entries, unique decoded keys, keys <=1,024 bytes, values <=256 KiB,
and **combined decoded key+value bytes <=256 KiB**. The existing 1 MiB JSON-frame
limit and engine resource budgets also apply. Invalid shapes, duplicate keys and
byte limits reject before any transaction is committed. Resource/queue exhaustion
can reject the whole batch before submission. An existing connection transaction
is preserved and the batch request rejected. Staged transactions consume the
existing global transaction budget; it is not silently multiplied for batches.
Clients handling larger values must split by bytes as well as count; this fixed
256-byte benchmark is well below the aggregate byte limit.

## Controlled fault model

All primary modes use the **same server image**, with
`service,backlog-batch,benchmark-metrics,sync-stall`. Only the client's ready-work
batch limit changes (1 or 8). Diagnostic stall control requires authentication,
both compile-time diagnostic features and explicit `CATDB_BENCH_STALL=1`.
It is a benchmark facility, not a default service API.

After all workers warm up and before offered traffic begins, worker zero arms:

| Scenario | Start after arm | Active window | Delay before a real sync | Maximum injections |
|---|---:|---:|---:|---:|
| normal | 1,100 ms | 0 | 0 | 0 |
| single | 1,100 ms | 10 s | 100 ms | 1 |
| burst | 1,100 ms | 1 s | 20 ms | 10,000 |

Traffic starts approximately 100 ms after the arm returns. The hook sleeps
immediately before the real log `sync_all`, with the normal publication boundary
still closed. It does not simulate torn writes or device error responses; separate
recovery/failure tests cover those. The burst is a fixed time window, not a fixed
number of syncs: policies may execute different numbers of injected syncs as
batch formation changes. Counts and actual sleep durations are recorded.

The diagnostic returns the first sleep start, last sleep end and cumulative
sleep elapsed nanoseconds relative to the server's arm instant. That instant
falls between the client's recorded arm-call start and completion. The report
uses this interval to bound the alignment of the actual last injected sleep
with client events, instead of assuming the requested sleep length was exact.
Arm/reset and counter snapshots are used only at quiescent boundaries. The
fields are independent reads, not an atomic concurrent statistics snapshot.

## Workload and outcomes

- 100,000 rows, 256-byte values, 8,000 independently scheduled requests per case.
- One connection: 200 and 250 offered requests/s; four: 400 and 800 total/s.
- Three repeats of both policies and all three fault scenarios: 72 cases.
- Mode order alternates each repeat; case order shuffles identically for each
  mode within a repeat. Fresh instance per mode/repeat.
- Two CPUs and 4 GiB each for Python client and Linux service containers.
- Original seeded key distribution, partitioned by worker; workers do not target
  the same key. Same-key/read/range conflict cases are tested separately.
- Deadlines do not move after stalls. Every offered update is completed, rejected
  or failed; unsuccessful outcomes are retained. A request's response latency is
  its planned time to completion, including all client wait.
- Group members share send-start/response-completion timestamps but retain their
  distinct scheduled/admission-attempt times, keys, value stamps and batch IDs. Latencies
  are per update, not per network message. Returns are independently verified.
- Full ordered content and empty/end boundaries are checked initially, after
  all 12 cases, and after restart: **84 checks over six instances**. Rejection
  means different successful subsets can occur; each is compared with its own
  successful-update oracle. No PALV or Z: data is used.

`admitted_ns` records the admission attempt immediately before credit acquisition,
not the exact grant instant. The semaphore enforces capacity. Offline validation
checks that a capacity-respecting assignment of grant times is possible between
attempt and send; it does not claim exact instantaneous occupancy from that field.

## Recovery metric and limitations

Recovery is the beginning of the first **100 ms offered-arrival window** after
the last injected sleep in which every scheduled update is admitted/completed
and has <=5 ms planned-to-send wait. The report publishes alignment bounds and
marks an unobserved recovery as censored, never as zero. All three trials appear
in the recovery plot. The table does not substitute a median of only the recovered
subset when another trial is censored. This is an explicit
queue-recovery threshold, not proof of zero backlog or restored device health.
The operation performing the final injected sleep still has to execute its real
sync; recovery therefore includes that completion and subsequent draining.

Whole-run p99/maxima include unrelated stalls. A supplemental neighborhood covers
the injection interval and two seconds afterward, including requests already
in flight at the first injection and all rejection outcomes in that neighborhood.
Matching offered rates does not force instantaneous send rates to match. Batching
changes network and synchronization cadence deliberately. The 100 ms stalled
request itself cannot be made faster by draining its successors. With 800/s and
64 credits, even one 100 ms global stoppage can necessarily exhaust admission
capacity; faster recovery is not a guarantee of zero rejection.

No per-request server trace or diagnostic pull runs during the timed workload.
Commit/sleep counters are read outside it. A supplemental normal-load cohort
compiled without `sync-stall` estimates the effect of the fault-hook machinery;
the common arrival scheduler/timestamp collector is not removed by that control.
It runs as a later cohort, so host-time differences confound the comparison; it
cannot establish a precise per-request hook cost.
Host/device activity remains uncontrolled, so small differences and sampled
maxima are not treated as causal guarantees. Process-crash tests are not physical
power-loss tests.

## Supplement: existing commit window

A separate paired cohort compares the ready-batch candidate with the same
candidate plus the existing `commit-window` feature: four connections at 800/s,
normal and repeated-delay scenarios, three repeats. Unlike the primary candidate,
this feature may briefly collect arrivals in the server and targets 32 pending
transactions under backlog. Its initial collection wait is 200 us; the existing
recent-cohort policy may use 1 ms. This is an explicit separate policy, not a
silent change to the primary no-extra-wait candidate. No single-connection or
single-injection result is inferred from this limited supplement.

## Reproduce

Use Docker Linux containers and Python 3; chart rendering also requires matplotlib. Networks, volumes and containers are
unique and disposable; existing output directories are refused. Do not overlap
timed cohorts with builds, tests, plotting or other benchmarks.

```sh
docker build -f benchmarks/Dockerfile -t catdb-comparison:local .
docker build -f benchmarks/backlog/Dockerfile --build-arg CATDB_FEATURES=service,backlog-batch,benchmark-metrics,sync-stall -t catdb-backlog:local .
docker build -f benchmarks/backlog/Dockerfile --build-arg CATDB_FEATURES=service,backlog-batch,benchmark-metrics -t catdb-backlog-clean:local .
python benchmarks/backlog/run.py --variant before=catdb-backlog:local --variant candidate=catdb-backlog:local --ops 8000 --reps 3 --out artifacts/backlog-comparison
python benchmarks/backlog/run.py --variant before-clean=catdb-backlog-clean:local --variant candidate-clean=catdb-backlog-clean:local --cases 1:250 4:800 --scenarios normal --ops 8000 --reps 3 --out artifacts/backlog-overhead
docker build -f benchmarks/backlog/Dockerfile --build-arg CATDB_FEATURES=service,backlog-batch,benchmark-metrics,sync-stall,commit-window -t catdb-backlog-window:local .
python benchmarks/backlog/run.py --variant candidate=catdb-backlog:local --variant candidate-window=catdb-backlog-window:local --cases 4:800 --scenarios normal burst --ops 8000 --reps 3 --out artifacts/backlog-window
python benchmarks/backlog/report.py artifacts/backlog-window docs/benchmarks/backlog-2026-11-05/window
python benchmarks/backlog/report.py artifacts/backlog-comparison docs/benchmarks/backlog-2026-11-05
python benchmarks/backlog/report.py artifacts/backlog-overhead docs/benchmarks/backlog-2026-11-05/overhead
cargo test --features service,backlog-batch,benchmark-metrics,sync-stall -- --test-threads=1
cargo build --features service,backlog-batch,benchmark-metrics,sync-stall --bin catdb-server
python benchmarks/backlog/test_protocol.py
python benchmarks/backlog/test_schedule.py
python benchmarks/backlog/test_report.py
```

The candidate batch size is selected by a variant name starting with `candidate`.
The clean image supports only `normal` in this runner; injected scenarios require
the explicit diagnostic API. Short pilot and smoke results are excluded from the
three-repeat comparison. The public result report states adoption and remaining
limitations; enabling the feature does not automatically change existing clients.
