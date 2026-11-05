from pathlib import Path
import json,gzip,shutil,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parents[2];raw=Path(sys.argv[1]);dest=root/'docs/benchmarks/flush-parent-2026-11-06';dest.mkdir(exist_ok=True)
rows=json.loads((raw/'results.json').read_text());summaries=json.loads((raw/'flush-summary.json').read_text())
invalid=[s for s in summaries if not s.get('valid')];summaries=[s for s in summaries if s.get('valid')]
assert len(rows)==12 and all(r['completed']==8000 and not r['errors'] and not r['rejected'] for r in rows)
fig,axes=plt.subplots(1,2,figsize=(10,4),layout='constrained')
table=[]
for i,c in enumerate([1,4]):
 for j,trace in enumerate([False,True]):
  rs=[r for r in rows if r['clients']==c and ('trace' in r['variant'])==trace]
  med=statistics.median([r['response']['p99_ms'] for r in rs]);maximum=max(r['response']['max_ms'] for r in rs)
  table.append((c,trace,med,maximum,statistics.median(r['ops_s'] for r in rs)))
  for ax,field in zip(axes,['p99_ms','max_ms']):
   vs=[r['response'][field] for r in rs];m=statistics.median(vs);x=i+(-.18 if not trace else .18)
   ax.bar(x,m,width=.32,color='#227b8e' if not trace else '#d78932',label=('Trace on' if trace else 'Trace off') if i==0 else None)
   ax.errorbar(x,m,yerr=[[m-min(vs)],[max(vs)-m]],fmt='none',color='black',capsize=4)
for ax,title in zip(axes,['Response p99','Per-run maximum response']):
 ax.set_title(title);ax.set_ylabel('Milliseconds');ax.set_xticks([0,1],['1 connection / 200/s','4 connections / 800/s']);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True);ax.legend()
fig.suptitle('Shared-flush observer overhead: 100k rows, 8k updates, 3 runs\nBars: run medians; whiskers: run min–max; unchanged DB engine',fontsize=11)
fig.savefig(dest/'observer-cost.svg');fig.savefig(raw/'observer-cost-preview.png',dpi=120);plt.close(fig)
p=dest/'observer-cost.svg';p.write_text('\n'.join(l.rstrip() for l in p.read_text().splitlines())+'\n')
for p in raw.iterdir():
 if p.is_file():shutil.copy2(p,dest/p.name)
 elif (p/'metadata.json').exists():shutil.copytree(p,dest/p.name,dirs_exist_ok=True)
shutil.copytree(raw.parent/'flush-parent-pilot',dest/'pilot',dirs_exist_ok=True)
shutil.copy2(raw.parent/'flush-parent-export-failure/result.json',dest/'export-failure-validation.json')
validation=[]
for p in (raw.parent/'flush-parent-validation').glob('*.txt'):
 s=p.read_text(encoding='utf-8-sig',errors='replace');validation.extend([p.name]+[l for l in s.splitlines() if 'test result:' in l or l.startswith('Ran ') or l=='OK'])
(dest/'validation.txt').write_text('\n'.join(validation)+'\n')
lines=['# Shared-flush parentage — 2026-11-06','',
'**The observer now follows journal-origin requests through worker-dispatched flush completion. This is attribution progress, not a database speedup.** No production implementation, durability setting or default changed.','',
'## Method and observer cost','',
'100,000 synthetic records × 256 bytes; 1 connection at 200 offered requests/s and 4 at 800/s; 8,000 updates per case, three repetitions, alternating off/on order. Both variants use the same profile-enabled server image and bounded client queues. This measures response under a fixed arrival rate, not maximum throughput. The preliminary 1,000-update runs are retained separately. All 96,000 main updates committed without rejection or operation error. Full-content verification follows each case and each restart.','',
'![Observer cost](observer-cost.svg)','',
'| Connections | Tracing | Median run p99 ms | Maximum across runs ms | Median updates/s |','|---:|---|---:|---:|---:|']
for c,t,p,m,o in table:lines.append(f'| {c} | {"on" if t else "off"} | {p:.3f} | {m:.3f} | {o:.1f} |')
lines+=['','## Directly observed parentage','',
'The collector adds flush insertion, sequence transition, completion entry and completion return probes. A parent is identified at insertion, including the current journal device and transaction. A pre/post-flush transition nested inside a particular completion callback establishes membership in that flush. Membership is a list, allowing multiple parents. Request slots, insertion serials and dispatch serials are distinct. Free events remove stale attribution. No nearest-time matching is used.','',
'The [Linux flush implementation](https://raw.githubusercontent.com/torvalds/linux/v6.18/block/blk-flush.c) advances waiting parent requests during the flush completion callback. That callback can also return early while references remain. The pilot observed such an empty callback followed by a member-bearing callback for the same dispatch; counting both as separate completed flushes would be wrong. These version-specific probes do not copy kernel implementation into catdb.','',
'| Case | Callbacks | Parent transitions | Journal assigned | Unknown | Multiple-parent callbacks | Unresolved dispatches |','|---|---:|---:|---:|---:|---:|---:|']
for s in summaries:lines.append(f"| {s['case']} | {s['callbacks']} | {s['members']} | {s['journal_members']} | {s['unknown_members']} | {s['shared_callbacks']} | {s['unresolved_dispatches']} |")
lines+=['', '**Trace-quality failure:** '+', '.join(s['case'] for s in invalid)+' was excluded from causal attribution because rq_update, rq_update_return and rq_free each reported one missed probe instance. All raw records remain published; there was no selective rerun. The timing plot includes all measured runs, including this case, but only five traces support the causal analysis. Complete three-repetition causal validation was not achieved.', '', 'Across the five accepted traces, 56,916 parent transitions were assigned to journals, with no unknown members or unresolved dispatches. No real multi-parent callback occurred; that branch is validated synthetically only.']
lines+=['','## Long waits and remaining limits','',
'| Case | Maximum fsync ms | Maximum journal ms | Journal runnable max ms | Maximum flush dispatch-to-callback ms |','|---|---:|---:|---:|---:|']
for s in summaries:lines.append(f"| {s['case']} | {s['worst_sync']['duration_ns']/1e6:.3f} | {s['worst_journal']['duration_ns']/1e6:.3f} | {s['max_journal_runnable_ns']/1e6:.3f} | {s['max_dispatch_to_callback_ms']:.3f} |")
lines+=['', 'The longest observed journal was 20.248 ms: 20.201 ms blocked, with directly attached pre-flush at 13.451 ms and post-flush at 1.908 ms from dispatch to callback. Those two flush spans account for about 76% of that journal interval. They do not explain the remaining interval or identify the host/device cause. Across accepted traces, the maximum journal runnable wait was 0.275 ms. This supports investigating the flush completion path before changing journal-thread scheduling, but does not justify disabling or weakening synchronization.']
lines+=['',
'Column maxima may belong to different events and must not be added or treated as one causal chain. `flush-summary.json` retains the worst journal and its directly attached flushes. Guest dispatch-to-callback time includes storage/host work and completion delivery; it is not physical device service time. This observer does not split that interval into Windows storage latency, virtual interrupt delivery and vCPU scheduling. It also does not prove that every guest-wide request belongs to catdb. Earlier 129-ms/1.47-s events are not retrospectively explained.','',
'Multiple-parent support, missing entries/returns, duplicate membership, preexisting requests, reuse, different CPU context and error retention have synthetic tests. Whether actual shared membership occurred is shown in the table; absent live branches are not claimed as live fault-injection coverage. The inherited request tests cover partial data completion and requeue.','',
'## Safety, bounds and reproducibility','',
'The private trace instance uses ten owned dynamic probes, a 16 MiB/CPU ring (512 MiB capacity on this guest), and a 180-second capture deadline. Loss counters and missed-probe counts are checked. Dumping/aliasing happens after timing; request and queue addresses are replaced by opaque aliases. Export-failure cleanup is tested; forced process/VM termination can still leave tracing resources. Production snapshots, conflicts, save failures and process-crash recovery are checked by the existing Rust/service/C ABI suites after measurement. Actual power-loss safety is not inferred from process-crash tests.','',
'No builds, tests or chart rendering overlap timed cohorts. Small host source/document edits and occasional reads of completed results occurred; unrelated host/device activity is not fully controlled. See [validation](validation.txt), [raw results](results.json), [exact parentage](flush-analysis.json.gz), [summary](flush-summary.json), [provenance](provenance.json), [pilot](pilot/results.json), and [reproduction](../../../benchmarks/flush-parent/README.md). Negative observer effects are retained.','']
(dest/'README.md').write_text('\n'.join(lines),encoding='utf-8')
p=root/'README.md';s=p.read_text(encoding='utf-8');h='## Follow-up: tracing journal parents through shared flush completion';assert h not in s
p.write_text(s+'\n'+h+'\n\nA bounded observer now records journal-to-flush membership across worker handoff. The same unchanged engine was measured with tracing off/on, 100k rows, 1/4 connections, 8k updates and three repetitions. This is diagnostic progress; no speedup or durability-policy change is claimed.\n\n![Shared-flush observer cost](docs/benchmarks/flush-parent-2026-11-06/observer-cost.svg)\n\nSee [parentage evidence, overhead, raw data and remaining limits](docs/benchmarks/flush-parent-2026-11-06/README.md).\n',encoding='utf-8')
