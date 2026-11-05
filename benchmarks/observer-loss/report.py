from pathlib import Path
import json,shutil,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parents[2];raw=Path(sys.argv[1]);dest=root/'docs/benchmarks/observer-loss-2026-11-06';dest.mkdir(exist_ok=True)
rows=json.loads((raw/'results.json').read_text());summary=json.loads((raw/'observer-summary.json').read_text());pilot=raw.parent/'observer-loss-pilot';ps=json.loads((pilot/'observer-summary.json').read_text())
assert len(rows)==12 and all(r['completed']==8000 and not r['errors'] and not r['rejected'] for r in rows)
groups=[];fig,axes=plt.subplots(1,2,figsize=(10,4),layout='constrained')
for i,c in enumerate([1,4]):
 for j,mode in enumerate(['full','minimal']):
  rs=[r for r in rows if r['clients']==c and r['variant'].endswith(mode)];ss=[s for s in summary if s['mode']==mode and f'-c{c}-' in s['case']]
  g=dict(clients=c,mode=mode,p99=statistics.median(r['response']['p99_ms'] for r in rs),maximum=max(r['response']['max_ms'] for r in rs),rejected_captures=sum(not s['gate'] for s in ss),events=statistics.median(s['event_count'] for s in ss),bytes=statistics.median(s['trace_bytes'] for s in ss),ops=statistics.median(r['ops_s'] for r in rs));groups.append(g)
  for ax,vs in zip(axes,[[s['trace_bytes']/1024**2 for s in ss],[r['response']['p99_ms'] for r in rs]]):
   m=statistics.median(vs);x=i+(-.18 if j==0 else .18);ax.bar(x,m,width=.32,color=['#357b8f','#d78932'][j],label=['15 probes','12 probes'][j] if i==0 else None);ax.errorbar(x,m,yerr=[[m-min(vs)],[max(vs)-m]],fmt='none',color='black',capsize=4)
for ax,title,unit in zip(axes,['Compressed trace size','Response p99'],['MiB per capture','Milliseconds']):
 ax.set_title(title);ax.set_ylabel(unit);ax.set_xticks([0,1],['1 connection / 200/s','4 connections / 800/s']);ax.legend();ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
fig.suptitle('Observer comparison: unchanged DB, 100k rows, 8k updates\nThree runs; bars = medians, whiskers = min–max',fontsize=11);fig.savefig(dest/'observer-comparison.svg');fig.savefig(raw/'observer-comparison-preview.png',dpi=120);plt.close(fig)
for p in dest.glob('*.svg'):p.write_text('\n'.join(l.rstrip() for l in p.read_text().splitlines())+'\n')
for p in raw.iterdir():
 if p.is_file():shutil.copy2(p,dest/p.name)
 elif (p/'metadata.json').exists():shutil.copytree(p,dest/p.name,dirs_exist_ok=True)
shutil.copytree(pilot,dest/'pilot',dirs_exist_ok=True)
shutil.copy2(raw.parent/'observer-loss-export-failure/result.json',dest/'export-failure-validation.json')
checks=[]
for p in (raw.parent/'observer-loss-validation').glob('*.txt'):
 s=p.read_text(encoding='utf-8-sig',errors='replace');checks.extend([p.name]+[l for l in s.splitlines() if 'test result:' in l or l.startswith('Ran ') or l=='OK'])
(dest/'validation.txt').write_text('\n'.join(checks)+'\n')
lines=['# Reducing observer work without waiving loss — 2026-11-06','',
'**Do not adopt the reduced observer as the default.** Compressed records fell about 24–29%, but the full observer had 0/6 rejected captures and the reduced observer 2/6. Median p99 also increased in both connection conditions. This changes an experimental observer, not the production DB; existing defaults and durability remain unchanged. The small sample does not prove that removing probes caused the misses.','',
'## Prior evidence and hypothesis','',
'The previous storage-stage study had zero ring overruns/drops in all six captures, but three captures had missed dynamic-probe instances. Entry and return probes on the affected paths reported equal missing counts. The [kernel implementation](https://raw.githubusercontent.com/torvalds/linux/v6.18/kernel/trace/trace_kprobe.c) combines entry-probe and return-instance misses in a return probe profile. Therefore, increasing return capacity alone cannot explain away entry-probe misses. Reentry is a candidate described by the [Kprobes documentation](https://docs.kernel.org/trace/kprobes.html), not a proven cause of these particular events.','',
'Counters are recorded before capture and after tracing stops. The strict check still rejects any cumulative miss, including setup activity; it does not subtract the baseline to waive losses. These counters do not locate the exact missed invocation, and their observation lifetime is broader than the timed client workload.','',
'The running kernel exposes CONFIG_OPTPROBES=y, CONFIG_KPROBES_ON_FTRACE=y and CONFIG_FUNCTION_TRACER=y. This establishes available mechanisms, not which mechanism caused a particular loss. All before-capture miss counters were zero in this study, all ring-loss counters remained zero, and no return probe showed extra misses beyond its paired entry probe. The exact triggering call/reentry remains unidentified.','',
'## Bounded preliminary comparison','',
'Each mode ran 1,000 updates once at 1/4 connections. All four traced modes had zero reported misses in that short pilot. No difference in loss reliability was demonstrated. Full = 15 probes; capacity = 15 with maxactive 128 for returns; minimal = 12, removing unused rq_end/rq_update/rq_update_return plus duplicate block/ext4 events. Standard = existing SCSI/block/journal events without dynamic probes.','',
'| Pilot case | Mode | Events in capture | Compressed bytes | Direct mapping gate |','|---|---|---:|---:|---|']
for s in ps:lines.append(f"| {s['case']} | {s['mode']} | {s['event_count']} | {s['trace_bytes']} | {s['gate']} |")
lines+=['',
'The standard-only mode lacks the same direct request-pointer/journal-parent mapping and was not advanced to causal testing. Increasing return capacity showed no distinguishing evidence in the short pilot, and prior misses also affected entry probes; that candidate was not extended. The minimal mode preserved direct mapping and reduced record volume, so it was selected for the repeated comparison with full mode. It intentionally loses the older per-data-update diagnostics, not the stage correspondence needed by this study.','',
'## Repeated comparison','',
'100,000 records × 256 bytes, 1 connection at 200 offered requests/s and 4 at 800/s, 8,000 updates per case, full/minimal alternating for three repetitions. Same server image, bounded request queues and durability settings. All 96,000 main updates committed with zero rejection/operation error. This is a fixed-arrival comparison of two observers, not maximum throughput and not a repeated trace-off comparison.','',
'![Observer comparison](observer-comparison.svg)','',
'| Connections | Mode | Median p99 ms | Maximum response ms | Rejected captures / 3 | Median events | Median compressed bytes |','|---:|---|---:|---:|---:|---:|---:|']
for g in groups:lines.append(f"| {g['clients']} | {g['mode']} | {g['p99']:.3f} | {g['maximum']:.3f} | {g['rejected_captures']} | {g['events']:.0f} | {g['bytes']:.0f} |")
lines+=['','| Capture | Missed probe instances by point | Baseline misses | Extra return misses over entry | Causal gate |','|---|---|---|---|---|']
for s in summary:lines.append(f"| {s['case']} | {s['misses']} | {s['before_misses']} | {s['extra_return_misses']} | {s['gate']} |")
lines+=['',
'Miss counts across different probe points are not distinct lost DB requests and must not be added as such. Rejected traces remain available but are excluded from causal interpretation. Client timing remains in the comparison even if tracing failed quality checks. Equal entry/return loss counts are consistent with a shared entry-side problem; they do not identify the exact reentry or interrupt path. No selective rerun or extended search was used.','',
'The 152.548-ms maximum response occurred in the second 1-connection minimal run, whose trace was rejected. It cannot be causally assigned to synchronization or the observer from that incomplete trace. Full-mode maximum response was 46.353 ms for 1 connection. These unfavorable results are retained, and no maximum-wait improvement is claimed.','',
'Record counts and compressed size cover the entire capture, not just the timed workload. Batch sizes and guest background I/O can also change event volume. Compression/export occur after timing; compressed bytes do not measure live RAM allocation or production WAL traffic. The ring remains 16 MiB/CPU (512 MiB capacity here), so a smaller exported file does not mean that reserved trace memory shrank.','',
'## Safety, limits and remaining work','',
'The exact probe set, zero cumulative misses, zero ring loss, cleanup, request identity/reuse, retries and complete stage matching remain required. Missing/bad counters and unmatched events are rejected. Standard-only trace success is not substituted for direct causal mapping. Host storage latency and virtual delivery remain unresolved; no database durability or default change is justified by these results alone.','',
'The observer uses a private instance, owned registry group and 180-second deadline. It does not modify global kernel tuning. Export failure cleanup is checked; forced process/VM termination can still leave resources. Pointer anonymization remains in place. Existing Rust/service/C ABI tests verify snapshots, conflicts and process-crash recovery after measurement; physical power loss is not tested. Full-content checks follow every condition and restart.','',
'No builds/tests/chart rendering overlapped timed runs. Small host source/document edits and completed-record reads occurred; other host/device activity was not controlled. See [reproduction](../../../benchmarks/observer-loss/README.md), [validation](validation.txt), [raw timing](results.json), [quality summary](observer-summary.json), [pilot](pilot/results.json) and [provenance](provenance.json).','']
(dest/'README.md').write_text('\n'.join(lines),encoding='utf-8')
p=root/'README.md';s=p.read_text(encoding='utf-8');h='## Follow-up: reducing observer work while retaining loss checks'
if h not in s:p.write_text(s+'\n'+h+'\n\nA 12-probe observer is compared with the previous 15-probe version. Record size, latency and rejected captures are reported separately; no missing-event check is waived and no production DB speedup is claimed.\n\n![Observer comparison](docs/benchmarks/observer-loss-2026-11-06/observer-comparison.svg)\n\nSee [the pilot, repeated comparison, loss evidence and limits](docs/benchmarks/observer-loss-2026-11-06/README.md).\n',encoding='utf-8')
