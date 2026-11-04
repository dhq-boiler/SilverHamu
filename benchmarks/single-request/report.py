"""Generate a report from the paired samples."""
import json,pathlib,statistics,sys
root=pathlib.Path(sys.argv[1]);rows=json.loads((root/'results.json').read_text(encoding='utf-8'))
def median(v,w,c,m):
 a=[r[m] for r in rows if r['variant']==v and r['workload']==w and r['clients']==c];assert len(a)==3
 return statistics.median(a)
text='''# Single-request service APIs and lightweight ping

Experimental warm-cache Python API measurements, 100,000 records / 256-byte
values. The regular append WAL, group-commit policy and durable acknowledgement
ordering are unchanged. Earlier product and fixed-log results are retained.

The same operation generator compares the old server, the new server's original
API, and the new server's one-request API. New calls perform the existing internal
transaction and validation path. Ping reads the version instead of computing
full database statistics. Neither change removes a durability barrier.

![Throughput and p99 for all variants](single-request.svg)

'''
text+=(root/'table.md').read_text(encoding='utf-8')
text+='\n## Changes relative to the original server\n\n| Operation | Clients | Throughput change | p99 change |\n|---|---:|---:|---:|\n'
for w in ['get','range100','update']:
 for c in [1,4]:
  q=(median('single',w,c,'ops_s')/median('before',w,c,'ops_s')-1)*100
  t=(median('single',w,c,'p99_ms')/median('before',w,c,'p99_ms')-1)*100
  text+=f'| {w} | {c} | {q:+.1f}% | {t:+.1f}% |\n'
text+='''
Three repeats alternate variant order; error bars are min/max, not confidence
intervals. The legacy series exposes regressions or noise in unchanged operations.
One-request scans have a fresh snapshot each request; this workload performs only
one 100-row scan per transaction in every variant. Stable multi-page reads still
use the original transaction API. No request deduplication or automatic retry is
added. A missing commit response can still mean a committed transaction.

## Safety and compatibility

- 432,000 timed operations, 72 full-content checks and nine service restarts:
  values and paired hashes agree; all raw latency samples are retained.
- Protocol tests cover empty/missing values, CRUD, paging, active-transaction
  rejection without discarding changes, bounds, old snapshots, conflicts and
  four concurrent single-request writers followed by process termination.
- The service dispatch test holds synchronization, checks no early success or
  new-value visibility, then confirms the old reader remains stable. Subprocess
  tests terminate the single-request path immediately before/after sync and
  check previously acknowledged values and complete recovered updates.
- Existing engine tests cover persistence failure, partial WAL, conflict detection,
  resource budgets and recovery through the same commit implementation. Windows,
  Linux, fixed-log compatibility, C ABI and container tests passed in
  [implementation CI](https://github.com/dhq-boiler/catdb/actions/runs/41928761537).
- Actual power loss and faulty storage were not simulated. This is not a new
  durability certification or an across-product performance ranking.

## Reproduction and limitations

[Commands and adapter details](../../../benchmarks/single-request/README.md),
[raw results](results.json), [configuration and image IDs](environment.json),
[numeric table](table.md), [source/test provenance](provenance.json), and [service API contract](../../service.md).

The original client generator is unchanged. The single adapter removes network
begin/rollback calls and sends one mutation at commit; its conversion and local
staging remain in operation timing. Full-content checks use the old API for all
variants. Containers have the same 2-CPU / 4-GiB limits on a shared workstation.
No other host build or benchmark was run concurrently. CPU quotas, Python thread
scheduling/GIL and Docker networking affect these results. These are closed-loop
operations, disjoint update keys, warm cache and no scheduled checkpoints. The
results do not cover overload, mixed workloads, cold startup, long-lived readers
or larger-than-memory data. PostgreSQL and Oracle were not rerun.
'''
ping=json.loads((root/'ping/results.json').read_text(encoding='utf-8'))
text+='\n## Auxiliary ping cost\n\nSame 100,000 records, one ping per timed operation, 1,000 requests per case;\nthree repeats per mode and client count. Unlike the old ping, the new ping never\ncounts records or retained versions. This is an isolated workload, not background\nhealthchecks mixed with application traffic.\n\n| Mode | Clients | Pings/s | p99 ms |\n|---|---:|---:|---:|\n'
for c in [1,4]:
 for v in ['before','after']:
  rr=[r for r in ping if r['variant']==v and r['clients']==c];assert len(rr)==3
  text+=f'| {v} | {c} | {statistics.median(r["ops_s"] for r in rr):.1f} | {statistics.median(r["p99_ms"] for r in rr):.3f} |\n'
text+='\n[Ping raw results](ping/results.json): 12,000 requests, 24 full-content checks\nand six restarts passed. Main-operation figures do not include these samples.\n'
(root/'README.md').write_text(text,encoding='utf-8')
