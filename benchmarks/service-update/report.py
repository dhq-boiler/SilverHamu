"""Build a compact results report from verified comparison and observer runs."""
import json
import pathlib
import statistics
import sys

root = pathlib.Path(sys.argv[1])
rows = json.loads((root / "results.json").read_text(encoding="utf-8"))
observer = json.loads((root / "observer/results.json").read_text(encoding="utf-8"))
diagnosis = json.loads((root / "observer/diagnosis.json").read_text(encoding="utf-8"))
def median(data, variant, metric, clients=4, workload="update"):
    subset = [r[metric] for r in data if r["variant"] == variant and r["clients"] == clients and r["workload"] == workload]
    assert len(subset) == 3
    return statistics.median(subset)
before = median(rows, "baseline", "ops_s")
after = median(rows, "handoff", "ops_s")
p99_before = median(rows, "baseline", "p99_ms")
p99_after = median(rows, "handoff", "p99_ms")
text = f"""# Service group-commit follow-up — 2026-11-05 (JST)

**Experimental.** This is a paired API microbenchmark on the same workstation,
not a general DBMS ranking or a power-loss certification.

## Result

With four clients, median throughput increased from **{before:.1f} to {after:.1f}
operations/s ({after / before:.2f}x)** and median run p99 decreased from
**{p99_before:.3f} to {p99_after:.3f} ms ({(1-p99_after/p99_before)*100:.1f}% lower)**.
Single-client update throughput was {median(rows, 'baseline', 'ops_s', 1):.1f}
versus {median(rows, 'handoff', 'ops_s', 1):.1f} operations/s; its p99 was
{median(rows, 'baseline', 'p99_ms', 1):.3f} versus {median(rows, 'handoff', 'p99_ms', 1):.3f} ms.
The full table and chart retain all read conditions and all repeat variability.

![Paired throughput and p99 for every condition](service-update.svg)

Bars are medians of three runs, whiskers min/max. p99 is calculated per run over
8,000 complete operations, then the three p99 values are summarized; samples
are not pooled. `max_ms` and all original samples remain in results and raw files.

{(root / 'table.md').read_text(encoding='utf-8')}

## Cause and change

The old service enabled batching, but did not enable the existing handoff
coordinator. Arrivals could drain the queue into tiny groups while the preceding
group was still synchronizing. Those small groups then queued for the writer
lock and each required another synchronization.

The `service` Cargo feature now depends on `group-handoff`. One coordinator
collects the next group after the preceding group finishes, allowing requests
arriving during synchronization to stay together. The 32-request / 4 MiB bounds,
conflict validation, log records, sync-before-publication and success-after-sync
semantics are preserved. The existing 200 us budget subtracts time already spent
queued. No async acknowledgement, new storage format, or external source-code
copy is introduced. Embedded builds without `service` retain their defaults.

## Timing decomposition and observer overhead

The separate observer experiment alternates four variants three times with
8,000 updates at four clients. The diagnostics include response serialization
and client-side recording costs. Main chart runs have instrumentation disabled.
Numbers below are medians of three per-run measurements, not evidence that
small differences are statistically significant.

| Variant | ops/s | p99 ms | commits per sync |
|---|---:|---:|---:|
"""
for variant in ("baseline", "baseline-diag", "handoff", "handoff-diag"):
    groups = [r["commits_per_sync"] for r in diagnosis if r["variant"] == variant]
    ratio = f"{statistics.median(groups):.3f}" if groups else "not instrumented"
    text += f"| {variant} | {median(observer, variant, 'ops_s'):.1f} | {median(observer, variant, 'p99_ms'):.3f} | {ratio} |\n"
text += "\n| Commit stage | Before, mean ms | After, mean ms |\n|---|---:|---:|\n"
for stage in diagnosis[0]["stages"]:
    values = [statistics.median(r["stages"][stage]["mean_ms"] for r in diagnosis if r["variant"] == variant) for variant in ("baseline-diag", "handoff-diag")]
    text += f"| {stage} | {values[0]:.4f} | {values[1]:.4f} |\n"
for variant in ("baseline", "handoff"):
    throughput_change = (median(observer, variant+'-diag', 'ops_s') / median(observer, variant, 'ops_s') - 1) * 100
    latency_change = (median(observer, variant+'-diag', 'p99_ms') / median(observer, variant, 'p99_ms') - 1) * 100
    text += f"\nFor {variant}, enabling diagnostics changed median throughput by {throughput_change:+.1f}% and p99 by {latency_change:+.1f}%.\n"
text += """
Instrumentation can alter arrival timing and group formation. An apparent
throughput gain with diagnostics is not evidence of negative overhead or zero
observer effect; the uninstrumented paired runs support the headline result.

`batch_queue` includes the coordinator handoff wait. `writer_wait` measures the
writer lock after group selection. Stage means, not stage p99s, explain the
commit path. `client_extra` includes network, parsing and scheduling and cannot
isolate network cost. Full-operation latency also includes begin and put.
See [all diagnostic distributions](observer/diagnosis.json),
[observer runs](observer/results.json), and the [trace definitions](../../../benchmarks/service-update/README.md).

## Conditions and verification

- Same original Python 3.12 client and operations: 100,000 records, 8-byte keys,
  256-byte values, 8,000 attempts, 1/4 clients, warm-up excluded. Closed-loop
  threads, disjoint update keys, warm cache. No scheduled concurrent checkpoint.
- Windows / Docker Desktop Linux (WSL2), Ryzen 9 5950X and approximately 128 GiB
  RAM. Each client/server has a 2-CPU quota and 4 GiB limit; no pinned affinity.
  Fresh named volumes on the same host. Shared workstation, not dedicated hardware.
- Baseline runtime sources: `1c22601` (identical engine to original comparison
  `60a67085f3f90cf8518106146d0c5a09e902ccbd`). Candidate uses this change's
  `service` feature at `a5dec6e`; see [build provenance](provenance.json).
  Immutable image IDs and source hashes are recorded in
  [environment.json](environment.json). Hashes describe the checkout at run start,
  not the historical baseline sources; variant images identify the actual binaries.
- Before/after order alternates, each six-case order is shuffled identically for
  the pair. All **288,000 operations**, **48 full-content checks**, including six
  restarts, passed. The separate observer run adds **96,000 operations** and
  **36 full-content checks**, including twelve restarts. No operation failed or
  was retried. Cross-variant all-value hashes match at every corresponding stage.
- Existing conflict, old-generation, count/byte limit, lone-request, sync failure
  and subprocess crash tests remain enabled. A deterministic test holds sync,
  checks that no uncommitted value or success is visible, then checks four
  commits in two syncs and exact values after reopen. Process crashes do not
  prove protection against power loss or storage firmware failures.

The [implementation CI](https://github.com/dhq-boiler/catdb/actions/runs/41882479578)
passed on Windows and Linux, including protocol, C ABI and Clippy checks; the
container job also passed. Locally, the Linux Rust suite, 13 real-process service
tests, 30 C ABI checks and the concurrent diagnostic-timestamp test passed.
The deterministic sync gate covers the added handoff behavior; existing tests
remain the oracle for conflict and recovery semantics.

## What open-source implementations contributed

Concepts were compared; third-party implementation code was not copied.
References were checked on 2026-11-05.

- [PostgreSQL 18 WAL configuration](https://www.postgresql.org/docs/18/wal-configuration.html)
  explains grouping arrivals during a preceding flush even without an added
  commit delay. [XLogFlush](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/access/transam/xlog.c)
  waits for WAL ownership and rechecks whether another process already flushed
  the needed position. This supports measuring group formation before increasing
  fixed waits; it is not a claim that catdb now implements PostgreSQL's WAL.
- [InnoDB 8.4 log writer/flusher source](https://github.com/mysql/mysql-server/blob/8.4/storage/innobase/log/log0write.cc)
  separates writing, durable flush progress and notification. catdb retains its
  existing coordinator because a separate background flusher would add another
  lifecycle and failure path; that larger change is not justified by this result.
- [SQLite WAL](https://www.sqlite.org/wal.html) permits readers alongside a writer,
  but only one writer at a time. It remains a useful durability and read-isolation
  reference, rather than a template for multiple concurrent writer admission.

## Limits and next question

The old PostgreSQL and Oracle results are historical context, not rerun in this
paired experiment. The improvement does not establish that catdb overtook them.
The original [all-product figures](../2026-11-05/README.md) remain unchanged and
show the old catdb implementation. Storage synchronization still dominates the
remaining update time; this change does not make each sync faster. Higher client
counts, hot-key conflicts, production checkpoint load, other devices and an
independent client implementation require separate evaluation. Do not extrapolate
these Python-client numbers to an engine-only throughput ceiling.

[Reproduction commands](../../../benchmarks/service-update/README.md) ·
[Main raw results](results.json) · [Numerical summary](summary.csv)
"""
(root / "README.md").write_text(text, encoding="utf-8")
