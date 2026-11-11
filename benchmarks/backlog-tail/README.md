# Attributing slow requests after bounded backlog draining

This follow-up preserves the previous ready-work batching policy and tests whether
remaining tail latency occurs in real log synchronization, group coordination,
or the client/service boundary. It does not equate a wall-clock sync interval
with physical disk service time: scheduling while inside the call is included.

## Measurement contract

The workload is the prior 100,000 synthetic records / 256-byte values, 8,000
offered requests, one connection at 200/250 requests per second or four at
400/800 total per second. Each connection immediately drains up to eight ready
distinct-key requests, using independent transactions and the same 64 total
outstanding credits. It never waits on the client for a group to fill. Same-key
requests keep their order; conflicts from other transactions remain possible.

Cases are normal, one 100 ms sleep before a real sync, and 20 ms sleeps before
syncs beginning in a one-second interval. Real synchronization and publication
ordering are unchanged. The same three repetitions alternate policy/image order
and shuffle case order. Each case has an exact successful-update oracle, verified
against all rows; each fresh instance is also verified after restart. Pilot runs
are explicitly excluded from the full comparison.

The copied workload files deliberately freeze the previous scheduling behavior
while adding post-workload record retrieval. They are not a new alternative
transaction implementation. Do not change pacing, capacity or retries when
comparing a new engine candidate.

## Deferred diagnostic capture

Build `service,backlog-batch,benchmark-metrics,sync-stall,lock-timing` and set
`CATDB_DIAGNOSTICS=buffer` only for the diagnostic experiment. The existing
streaming/pull modes remain available. Default builds do not expose this mode.

- Each authenticated connection assigns an index to every ordinary request.
  `diagnostic_drain` is excluded from the index. The client tracks the same
  sequence including warmup, counter and arm requests.
- After sending a commit response, retain a record only when decoded-dispatch
  start through the post-send timestamp is at least 10 ms. No diagnostic payload
  or additional network round trip is inserted in a timed update.
- The buffer holds at most 8,192 slow network requests per connection, each with
  at most eight transaction records. Saturation increments a loss counter; it
  does not allocate indefinitely or block a commit waiting for a consumer.
- After all workers drain their updates and counters are collected, each reads
  chunks of at most 128 records. Retrieval is outside measured response times.
  The benchmark rejects any lost record or unmatched request index.
- This is a bounded diagnostic allocation, not part of the engine data budget;
  multiple connections multiply its memory cost. Buffers disappear on disconnect.
  A failed socket send can prevent record retention/retrieval. This is not an
  audit log or proof that disconnected requests rolled back.

For a batch, the parent record carries the request index and dispatch interval;
item order matches input order. The reused item envelope has zero placeholders
for request_index, dispatch_ns and sync_attempts, not measured zero durations or
zero sync calls. Attribution uses the parent identity and item trace/detail arrays.

Each individual batch result retains its own submission, group trace, reply-ready
and reply-received timestamps. Capturing these does not combine the transactions
into an atomic all-or-nothing batch. The success of one entry does not imply the
success of every entry; uncertain responses still require the existing handling.

## Timing interpretation

The trace distinguishes request preparation, pending-lock wait, leader/preceding
group wait, collection, writer-lock wait, validation/reservation, write, pre-sync
hook, actual `sync_all`, publication, release/reply delivery, response construction
and send. The marker immediately before `sync_all` separates injected sleep from
the actual syscall interval. The pre-sync interval includes hook/timestamp costs,
not only requested sleep duration.

The pending-lock interval also includes enqueue work before that item is inserted;
the leader interval includes reacquiring the pending mutex. These are wall-time
coordination intervals, not isolated mutex contention counters. Validation/build
and reservation, state-lock acquisition and publication-lock acquisition are
separated where the existing markers permit.

Collection start can precede a follower's submission. Its collection contribution
therefore starts at the later of those two timestamps; this avoids counting the
same preceding-group wait twice. A group's write/sync/publication spans appear in
all member transactions; do not sum them as independent disk operations. Phase
percentiles also cannot be added to reconstruct an end-to-end percentile.

Client planned-to-send time includes arrival scheduling and the finite FIFO.
Send-to-reply includes client encoding/parsing, transport and server work. The
remainder after subtracting server wall time is labeled a residual, not network
latency. Server post-send timestamping can occur after the client receives a reply
if that thread is descheduled, so this residual can be negative. Server and client
monotonic clock origins are not treated as interchangeable.

If a maximum client response has no retained server record, the report does not
invent its phases. Its own server interval was below the retention threshold
assuming complete retrieval, but a slow predecessor may have created its queue
wait. Records for predecessors remain available by the same connection index.
The previous historical 1.47 s request cannot be retroactively attributed: only
new instrumented observations support phase-level conclusions.

## Observer control and reproduction

Compare a build without `lock-timing` against the instrumented build at the same
offered rates, rather than assuming capture is free. This measures the combined
effect of trace markers and buffered retention, not just JSON serialization.
Both include the common client timestamps and sync-fault facility. Keep host
builds, test suites and chart rendering outside timed cohorts.

```sh
docker build -f benchmarks/Dockerfile -t catdb-comparison:local .
docker build -f benchmarks/backlog-tail/Dockerfile --build-arg CATDB_FEATURES=service,backlog-batch,benchmark-metrics,sync-stall -t catdb-tail-clean:local .
docker build -f benchmarks/backlog-tail/Dockerfile --build-arg CATDB_FEATURES=service,backlog-batch,benchmark-metrics,sync-stall,lock-timing -t catdb-tail-profile:local .
python benchmarks/backlog-tail/run.py --variant candidate-off=catdb-tail-clean:local --variant candidate-profile=catdb-tail-profile:local --ops 8000 --reps 3 --out artifacts/backlog-tail-comparison
python benchmarks/backlog-tail/analyze.py artifacts/backlog-tail-comparison
cargo test --features service,backlog-batch,benchmark-metrics,sync-stall,lock-timing -- --test-threads=1
cargo build --features service,backlog-batch,benchmark-metrics,sync-stall,lock-timing --bin silverhamu-server
python benchmarks/backlog-tail/test_protocol.py
python benchmarks/backlog-tail/test_analysis.py
```

Variant names starting with `candidate` use the ready-work batch limit of eight;
names containing `profile` enable buffered diagnostics. These naming conventions
are explicit benchmark controls, not automatic production behavior. Linux Docker
containers use unique disposable networks/volumes with 2 CPUs and 4 GiB each.
No production library or other database product is used in this follow-up.
