# Constrained backlog pilot: predeclared gate

Same image, aged 1M rows, 256-byte values, 1 CPU/512 MiB, old reader,
read + 4 writers, 2,000 requests per read/mixed phase, 1,000 total arrivals/s.
64 outstanding write credits (16 per writer) and 64 read credits include queued,
carried and active requests. Both policies use the same credits and key-to-worker
routing. The existing limit-8 API receives only immediately available distinct keys.

Four pilot conditions: normal/1, normal/8, single-100ms/8, single-100ms/1.
Real sync remains; the one injected stall is armed for 500 ms into mixed traffic.

Proceed only if all correctness/old-generation/reopen/ownership/resource checks
pass and all these pilot gates pass:

- Normal mixed read p99 <= max(baseline * 1.25, baseline + 0.2 ms).
- Normal write p99 <= max(baseline * 1.25, baseline + 1 ms).
- Normal and single refusal counts do not increase; no operation errors or OOM.
- Both sampled RSS peaks <= baseline * 1.05.
- Single-stall sync count is strictly lower.
- Single-stall outstanding-backlog recovery is >=10% shorter, with write p99
  and maximum no more than 10% worse.

Recovery means elapsed time from the first maximum outstanding-write count in
[arm_begin+500ms, arm_end+1000ms] to the first return to <=4 outstanding writes.
An outstanding write ends only at its completed response, not when sent or dequeued.
This client-clock window brackets the injected event but does not claim an exact
server/client clock mapping. No peak above 4 or no recovery makes the gate
inconclusive, not passed. Raw records retain all admitted/rejected requests.

These are screening thresholds, not statistical proof. A failing/inconclusive
pilot ends this experiment; do not extend the search or change the default.
Only a passing gate permits the approved 8,000-request repeated comparison.

## Budget-partitioned follow-up

`python benchmarks/constrained-backlog/run.py --out artifacts/budget-pilot --budget-aware`
reuses the same image, arrival schedule, credits, and screening gate. The requested
batch ceiling stays eight; the effective ceiling is computed before starting:
`min(requested, (staged_budget // 4) // (128 + key_bytes + value_bytes))`.
For this fixed 8-byte key/256-byte value workload and 4096-byte budget it is two.
Each writer keeps its quota until the synchronous response, so the four clients
prepare at most eight writes (3136 charged bytes). No fill delay is introduced.
An entry too large for one writer's quota is rejected during setup.

This is client-side partitioning for four controlled writers, not an engine-wide
reservation API. Other writers can still exhaust the budget. General variable-size
batches and borrowing unused quotas are out of scope. The server still enforces its
original limit; queued-but-unsent requests remain included in measured latency and
outstanding-request credits. Fewer refusals alone do not count as a speedup.
