"""Write a report from validated main and observer measurements."""
import gzip,json,pathlib,statistics,sys
root=pathlib.Path(sys.argv[1]);rows=json.loads((root/'results.json').read_text(encoding='utf-8'));env=json.loads((root/'environment.json').read_text(encoding='utf-8'))
obs=json.loads((root/'observer/results.json').read_text(encoding='utf-8'));diagnosis=json.loads((root/'observer/diagnosis.json').read_text(encoding='utf-8'))
def median(variant,clients,metric):
    v=[r[metric] for r in rows if r['variant']==variant and r['clients']==clients];assert len(v)==3
    return statistics.median(v)
def om(variant,metric):return statistics.median(r[metric] for r in obs if r['variant']==variant)
a=median('current',4,'ops_s');b=median('fixed',4,'ops_s');ap=median('current',4,'p99_ms');bp=median('fixed',4,'p99_ms')
text=f'''# Fixed-length WAL prototype results

The prototype is **opt-in; the default append log is unchanged**. It preinitializes
64 MiB of data capacity and keeps logical-end records on two separate 4 KiB
pages. A successful commit waits for **both payload sync and end-record sync**.
The earlier file-only result near 1.6 ms is not claimed as end-to-end commit time.

For 100,000 records and 256-byte values, the three-run four-client median changes
from **{a:.1f} to {b:.1f} updates/s ({(b/a-1)*100:+.1f}%)** and from
**{ap:.3f} to {bp:.3f} ms p99 ({(bp/ap-1)*100:+.1f}%)**.
These small-sample workstation observations are not a general ranking or proof
of a repeatable win on other storage. The modest change, extra writes/reserved
space and new recovery format do not justify replacing the standard log here.

![Append log versus fixed-length prototype](fixed-log.svg)

'''
text+=(root/'table.md').read_text(encoding='utf-8')
text+='''
All cases use the unchanged original Python operation implementation. Three
repeats alternate mode order and share the same shuffled client-count order.
The chart has instrumentation disabled and shows all three-run ranges.
Both modes retain the previous service batching policy. PostgreSQL and Oracle
were not rerun; their earlier charts remain unchanged.

## Synchronization and observer overhead

Instrumented and ordinary builds are measured separately at four clients, three
repeats each. Server timing, diagnostic JSON and client collection can change
arrival/group timing; instrumentation results do not replace the main chart.

| Build | Updates/s | p99 ms |
|---|---:|---:|
'''
for v in ['current','current-diag','fixed','fixed-diag']:text+=f'| {v} | {om(v,"ops_s"):.1f} | {om(v,"p99_ms"):.3f} |\n'
text+='\n| Instrumented mode | Updates / durability group | Sync calls / group | Complete sync interval, mean ms |\n|---|---:|---:|---:|\n'
for v in ['current-diag','fixed-diag']:
    means=[]
    for row in obs:
        if row['variant']!=v:continue
        with gzip.open(root/'observer'/row['profile_file'],'rt') as f:events=json.load(f)
        groups={e['server']['trace'][6]:e['server']['trace'][6]-e['server']['trace'][5] for e in events if e['op']=='commit' and e['server']}
        means.append(statistics.mean(groups.values())/1e6)
    ds=[d for d in diagnosis if d['variant']==v];assert len(ds)==3 and len(means)==3
    text+=f'| {v} | {statistics.median(d["commits_per_sync"] for d in ds):.3f} | {2 if v=="fixed-diag" else 1} | {statistics.median(means):.3f} |\n'
fixed=[r for r in obs if r['variant']=='fixed-diag']
payload=statistics.median(r['fixed_sync_metrics'][1]/r['fixed_sync_metrics'][0]/1e6 for r in fixed)
end=statistics.median(r['fixed_sync_metrics'][2]/r['fixed_sync_metrics'][0]/1e6 for r in fixed)
text+=f'''
For the fixed prototype, median run means are **{payload:.3f} ms for payload sync**
and **{end:.3f} ms for end-record sync**. The complete interval also includes
sentinel/end-record writes, seeks and bookkeeping. Both syncs are required by this
prototype's recovery protocol; neither is removed from the response timer.

Means above are per durability group (deduplicated from commit traces), then
medians across runs. Existing [stage distributions](observer/diagnosis.json) are
per-commit weighted, and cannot be summed at p99. All raw traces and aggregate
payload/end sync counters are retained in [observer results](observer/results.json).

## Write cost, capacity and reopen

Application log bytes count all frame writes plus 20 sentinel bytes and 64
end-record bytes per fixed commit group. They **exclude initial zero filling and
checkpoint replacement**, and are not physical-device write amplification.
The main and observer runs report application bytes. An initially empty cgroup
`io.stat` was mistaken for unavailable counters; a 4 MiB synchronized-write probe
confirmed they work. The separate block-I/O comparison below corrects that assumption.

The prototype initializes a 64 MiB data region and 8 KiB metadata, including
unused space. That extra initial write/storage cost is not included in timed
steady-state updates. An explicit checkpoint is required to expand capacity;
there is no in-place growth or automatic space recycling in this prototype.

| Mode | Final physical file bytes | Warm-cache Database::open ms (median; min–max) |
|---|---:|---:|
'''
for v in ['current','fixed']:
    runs=[r for r in env['runs'] if r['variant']==v]
    times=[r['reopen_probe']['open_ns']/1e6 for r in runs]
    sizes=[r['io_after']['physical_bytes'] for r in rows if r['variant']==v and r['clients']==1]
    # Use each repeat's last case because the client-count order is shuffled.
    sizes=[[r for r in rows if r['variant']==v and r['rep']==run['rep']][-1]['io_after']['physical_bytes'] for run in runs]
    text+=f'| {v} | {statistics.median(sizes):,.0f} | {statistics.median(times):.3f}; {min(times):.3f}–{max(times):.3f} |\n'
text+="""
For the reopen table above, the common prototype reader opens each file after both update cases, with the
server stopped. Timing excludes Docker/process launch, subsequent stats and
full-content checks. It includes replay and the fixed format's repair syncs.
These are warm-cache reader timings, not cold startup or UI readiness.
"""

io=json.loads((root/'io-accounting/results.json').read_text(encoding='utf-8'))
text+='\n## Supplemental block-I/O accounting\n\nSame four-client workload, three repeats per mode, 8,000 updates per case. These\nare server-cgroup-attributed block bytes and requests, not complete physical-media\nwrite amplification. Initialization/conversion and shared seeding are outside these\ndeltas. Counter settling is outside the operation timer. These auxiliary timings\ndo not replace the main comparison.\n\n| Mode | Block bytes / case | Write requests / case | Updates/s | p99 ms |\n|---|---:|---:|---:|---:|\n'
for v in ['current','fixed']:
    rr=[r for r in io if r['variant']==v];assert len(rr)==3
    text+=f'| {v} | {statistics.median(r["io_delta"]["block_write_bytes"] for r in rr):,.0f} | {statistics.median(r["io_delta"]["block_write_requests"] for r in rr):,.0f} | {statistics.median(r["ops_s"] for r in rr):.1f} | {statistics.median(r["p99_ms"] for r in rr):.3f} |\n'
text+='\n[Supplemental raw results](io-accounting/results.json) retain all samples and\ninitialization counters. All 48,000 operations, 18 content checks and six reopens\npassed. Some journal/controller work can be attributed elsewhere.\n'

text+='''

## Safety and compatibility

The [format/recovery design](../../fixed-log.md) explains the two-barrier protocol,
end-record checksums, confirmed-region validation and incomplete-tail handling.
A single damaged metadata copy can be recovered by validating complete payload;
both damaged copies cause an explicit open failure. Complete corruption inside
the confirmed region is rejected without modifying the file. Simultaneous
arbitrary payload/metadata damage is not guaranteed recoverable.

Validation includes old snapshots, read/write/range conflicts, exact capacity and
checkpoint expansion, partial frame/end writes, individual metadata-page loss,
allocation/sync failure, no success or new visibility after payload sync alone,
and process exits at both barriers and checkpoint publication. Complete uncertain
transactions may appear after recovery; partial transactions may not.

- Main comparison: 96,000 operations, zero errors/retries, 24 full-content checks,
  six timed open probes and six service restarts. Paired hashes agree.
- Observer comparison: 96,000 operations, 36 full-content checks, 12 timed opens
  and 12 service restarts; all expected commit traces are present and monotonic.
- Final layout: 11 dedicated Rust tests plus two ignored subprocess helpers;
  Linux service protocol (including four-writer kill/restart): 13 passed;
  C ABI compatibility checks: 30 passed; clippy passed.
- [Implementation CI](https://github.com/dhq-boiler/catdb/actions/runs/41896592381)
  tests standard and fixed-log builds on Windows/Linux, including service and
  C ABI compatibility, plus the standard Docker lifecycle.

Actual power cuts and arbitrary storage faults were not tested. Durability
requires OS/device synchronization to work as specified. This remains Experimental;
no real library was migrated and ordinary builds still create the original format.
A `fixed-log` build can open old logs; builds without it reject the new format.

## Retained explorations and reproduction

The [adjacent-record preliminary comparison](pilots/adjacent-end-records/results.json)
is retained with all samples. That earlier layout placed both end records in the
same page; it was replaced by separate pages rather than treated as independent
recovery copies. Its 1,000-operation diagnostic preflight, source snapshot and
setup notes are in [pilots](pilots/README.md). It is not the final benchmark.

[Reproduction commands](../../../benchmarks/fixed-log/README.md),
[raw main results](results.json), [images/configuration](environment.json),
[source/test provenance](provenance.json) and [numeric table](table.md) accompany
the figures. Original comparison data was not overwritten. No mixed-read,
concurrent-checkpoint, cold-cache, larger-than-memory or long-duration exhaustion
claim is made from this update-only experiment.
'''
(root/'README.md').write_text(text,encoding='utf-8')
