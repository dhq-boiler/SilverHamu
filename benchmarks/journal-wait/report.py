import pathlib,json,gzip,shutil,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=pathlib.Path(__file__).resolve().parents[2];raw=pathlib.Path(sys.argv[1]);dest=root/'docs/benchmarks/journal-wait-2026-11-06';dest.mkdir(exist_ok=True)
rows=json.loads((raw/'results.json').read_text());env=json.loads((raw/'environment.json').read_text());summary=json.loads((raw/'summary.json').read_text())
assert len(rows)==4 and all(r['completed']==1000 and not r['errors'] and not r['rejected'] for r in rows)
assert all(r['status']=='passed' for r in env['runs'])
fig,ax=plt.subplots(figsize=(10,3.4),layout='constrained');left=[0.,0.]
for field,label,color in [('blocked_ns','Blocked (I/O ownership unresolved)','#236b8e'),('runnable_ns','Runnable until scheduled','#e78b29'),('scheduled_ns','Scheduled (includes IRQ / VM effects)','#67a976')]:
 vals=[s['worst'][field]/1e6 for s in summary];ax.barh(range(2),vals,left=left,label=label,color=color);left=[x+y for x,y in zip(left,vals)]
ax.set_yticks(range(2),['4 connections / 800 requests/s','1 connection / 200 requests/s']);ax.set_xlabel('Time inside the longest observed journal transaction (ms)');ax.set_title('Short feasibility pilot — 1,000 updates per case, not a performance comparison');ax.legend(fontsize=8,loc='lower right');ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True)
fig.savefig(dest/'journal-waits.svg');fig.savefig(raw/'journal-waits-preview.png',dpi=120);plt.close(fig)
p=dest/'journal-waits.svg';p.write_text('\n'.join(l.rstrip() for l in p.read_text().splitlines())+'\n')
for p in raw.iterdir():
 if p.is_file():shutil.copy2(p,dest/p.name)
 elif (p/'metadata.json').exists():shutil.copytree(p,dest/p.name,dirs_exist_ok=True)
shutil.copytree(raw.parent/'journal-wait-pilot',dest/'invalid-collector-attempt',dirs_exist_ok=True)
lines=['# Journal-wait feasibility investigation — 2026-11-06','',
'**The feasibility gate did not fully pass.** Journal transaction phases and its thread scheduling can be paired, but these block tracepoints do not provide a stable request ID or a journal-transaction association. Physical/virtual I/O service time cannot be inferred from overlapping events alone. Following the agreed stopping rule, the 8,000-request / three-repetition main comparison was **not run**. No performance improvement or default change is claimed.','',
'## What was measured','',
'The existing unchanged server image and workload used 100,000 records of 256 bytes, 1 connection at 200 offered requests/s and 4 at 800/s. Each condition offered only 1,000 updates, once with tracing off and once with tracing on. The observer records journal start/locking/flushing/logging/end, journal/DB thread switch and wakeup, ext4 sync and block issue/complete/merge/requeue events. Per-switch stack acquisition was removed. Its ring capacity is still 16 MiB per CPU (512 MiB on this guest), so lower maximum memory use is not claimed.','',
'![Journal wait partitions](journal-waits.svg)','',
'| Pilot condition | Journal transactions | Longest journal transaction (ms) | Blocked (ms) | Runnable (ms) | Scheduled (ms) |','|---|---:|---:|---:|---:|---:|']
for s in summary:
 w=s['worst'];lines.append(f"| {s['case']} | {s['journals']} | {w['duration_ns']/1e6:.3f} | {w['blocked_ns']/1e6:.3f} | {w['runnable_ns']/1e6:.3f} | {w['scheduled_ns']/1e6:.3f} |")
lines+=['',
'The 1,279 journal transactions and 1,279 fsync calls were complete, with no reported overrun, dropped events, missing wakeup or phase-order error. Equal counts are a coverage check, **not proof of ownership between a particular DB sync and journal transaction**. The longest journal transaction was 12.045 ms: 11.999 ms blocked, 0.008 ms runnable and 0.038 ms scheduled. Across journal transactions the largest runnable interval total was 0.087 ms. The previous 129-ms outlier did not recur in this short pilot; its internal cause remains unassigned.','',
'## Why the I/O attribution gate failed','',
'The observed device was 8,64. Flush issue events used sector 0, while corresponding-looking flush completions used sector 18446744073709551615. Journal commit writes also emitted a nonzero-sector-count completion followed by a zero-sector-count completion. Pairing by nearest timestamp, sector, or one issue/one completion would therefore be unsafe. Other writes share this device and work can move from the journal thread to a worker.','',
'The kernel [block tracepoint definition](https://raw.githubusercontent.com/torvalds/linux/v6.18/include/trace/events/block.h) exposes device, range and operation fields but no request pointer in this output; a completion can represent a portion of a request. The [journal commit path](https://raw.githubusercontent.com/torvalds/linux/v6.18/fs/jbd2/commit.c) also involves commit-record submission and waiting, with flush/FUA behavior depending on filesystem flags. These upstream definitions guide interpretation; this run uses the WSL2 kernel recorded in the preceding study, and its actual event formats were inspected.','',
'We therefore report journal **blocked time**, not proven disk time. We cannot yet split that time between virtual-device completion, worker scheduling and shared filesystem work. The next instrumentation would need stable request identity across submission, split/merge/requeue/completion plus a defensible journal association; host Windows/device tracing would be needed to investigate below the guest. Neither stronger tracing nor filesystem policy changes were silently introduced in this pilot.','',
'## Observer effect: preliminary only','',
'| Connections / offered rate | Trace | p99 response (ms) | Maximum response (ms) | Updates/s |','|---|---|---:|---:|---:|']
for r in rows:lines.append(f"| {r['clients']} / {r['rate']} | {'on' if 'trace' in r['variant'] else 'off'} | {r['response']['p99_ms']:.3f} | {r['response']['max_ms']:.3f} | {r['ops_s']:.1f} |")
lines+=['',
'One short run is insufficient to quantify tracing overhead or a speedup; unfavorable results are retained. Removing stacks reduces one source of work, but this pilot does not demonstrate negligible observer overhead. There were 4,000 committed updates, no rejection or operation failure, eight full-content checks including two restarts. No benchmarks overlapped the safety test suites.','',
'The first pilot accidentally mounted the previous collector. The missing journal event set was detected, the mount and an explicit event-set assertion were fixed, and its data are retained under `invalid-collector-attempt` rather than used for journal conclusions.','',
'## Safety and reproduction','',
'No production Rust/service/C ABI implementation or durability policy changed. Existing safety tests cover old snapshots, conflicts, failure and process-crash recovery; the new analyzer rejects incomplete/contradictory journal histories. See [validation.txt](validation.txt). Process-crash tests do not prove physical power-loss durability.','',
'See [the harness](../../../benchmarks/journal-wait/README.md), [all pilot results](results.json), [exact journal partitions and block context](journal-analysis.json.gz), [summary](summary.json), [environment](environment.json), and [source/image provenance](provenance.json). Both owned trace instances were stopped and removed. Block events are context only, never silently converted to per-journal I/O durations.','']
(dest/'README.md').write_text('\n'.join(lines))
p=root/'README.md';s=p.read_text();heading='## Follow-up: journal scheduling and the I/O attribution limit'
assert heading not in s
s+='\n'+heading+'\n\nA short pilot separated journal-thread scheduling, but did not establish ownership of shared/split block I/O. The long comparison was stopped at its feasibility gate; no speedup or durability change is claimed.\n\n![Journal wait feasibility pilot](docs/benchmarks/journal-wait-2026-11-06/journal-waits.svg)\n\nSee [the measured boundaries, preliminary observer results and raw evidence](docs/benchmarks/journal-wait-2026-11-06/README.md).\n';p.write_text(s)
