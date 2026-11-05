import gzip,json,pathlib,shutil,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=pathlib.Path(__file__).resolve().parents[2];raw=pathlib.Path(sys.argv[1]);dest=root/'docs/benchmarks/sync-os-2026-11-05'
dest.mkdir(exist_ok=True)
results=json.loads((raw/'results.json').read_text());env=json.loads((raw/'environment.json').read_text())
with gzip.open(raw/'kernel-analysis.json.gz','rt') as f:kernel=json.load(f)
assert len(results)==24 and all(r['attempted']==8000 and not r['errors'] for r in results)
assert len(kernel)==12 and all(r['status']=='passed' for r in env['runs'])
variants=['candidate-profile-off','candidate-profile-trace'];cases=[(1,200),(1,250),(4,400),(4,800)]
colors=['#236b8e','#e78b29'];labels=['Tracing off','Kernel tracing + blocked stacks']
fig,axes=plt.subplots(1,3,figsize=(14,4.3),layout='constrained')
for ax,(field,title) in zip(axes,[('p99_ms','Response p99 (ms)'),('max_ms','Worst response per run (ms)'),('ops_s','Completed updates / second')]):
 for j,v in enumerate(variants):
  groups=[[r for r in results if r['variant']==v and (r['clients'],r['rate'])==c] for c in cases]
  assert all(len(g)==3 for g in groups)
  ys=[[r['ops_s'] if field=='ops_s' else r['response'][field] for r in g] for g in groups]
  med=[statistics.median(y) for y in ys];x=[i+(j-.5)*.34 for i in range(4)]
  ax.bar(x,med,.32,label=labels[j],color=colors[j]);ax.errorbar(x,med,yerr=[[m-min(y) for m,y in zip(med,ys)],[max(y)-m for m,y in zip(med,ys)]],fmt='none',ecolor='#333333',capsize=3)
 ax.set_xticks(range(4),['1 / 200','1 / 250','4 / 400','4 / 800']);ax.set_xlabel('Connections / offered requests per second');ax.set_title(title);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
fig.suptitle('Same server binary; 100,000 rows × 256 B; 8,000 requests; 3 runs / condition')
axes[0].legend(fontsize=8);fig.savefig(dest/'observer.svg');fig.savefig(raw/'observer-preview.png',dpi=120);plt.close(fig)
all_sync=[dict(case=k['case'],**s) for k in kernel for s in k['syncs']]
top=sorted(all_sync,key=lambda s:s['duration_ns'],reverse=True)[:12]
fig,ax=plt.subplots(figsize=(11,6),layout='constrained');left=[0.0]*len(top)
for field,label,color in [('io_stack_blocked_ns','Filesystem / I/O-path blocked','#236b8e'),('other_blocked_ns','Other / unclassified blocked','#999999'),('runnable_ns','Runnable → scheduled','#e78b29'),('scheduled_ns','Scheduled (includes IRQ / VM effects)','#67a976')]:
 vals=[s[field]/1e6 for s in top];ax.barh(range(len(top)),vals,left=left,label=label,color=color);left=[a+b for a,b in zip(left,vals)]
ax.set_yticks(range(len(top)),[f"#{i+1} "+s['case'].replace('candidate-profile-trace-','')+f" / tid {s['tid']}" for i,s in enumerate(top)],fontsize=8);ax.invert_yaxis();ax.set_xlabel('Time inside one fsync syscall (ms)');ax.set_title('Twelve longest observed sync calls — guest kernel attribution');ax.legend(fontsize=8,loc='lower right');ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True);fig.savefig(dest/'sync-tail.svg');fig.savefig(raw/'sync-tail-preview.png',dpi=120);plt.close(fig)
for svg in dest.glob('*.svg'):
 svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines())+'\n')
for p in raw.iterdir():
 if p.is_file():shutil.copy2(p,dest/p.name)
 elif (p/'metadata.json').exists():shutil.copytree(p,dest/p.name,dirs_exist_ok=True)
if len(sys.argv)>2:shutil.copytree(pathlib.Path(sys.argv[2]),dest/'initial-attempt',dirs_exist_ok=True)
total=sum(r['attempted'] for r in results);done=sum(r['completed'] for r in results);rejected=sum(r['rejected'] for r in results);checks=sum(len(e['checks']) for e in env['runs']);worst=top[0]
lines=['# Log-sync I/O-path versus runnable delay','',
'The server, synchronization primitive and commit policy are unchanged. This is an observer-effect comparison and a kernel attribution study, **not a demonstrated reduction in maximum latency**. No earlier 1.47-second event is assigned a cause retrospectively.','',
'## Conditions','',
'100,000 synthetic records, 256-byte values, 8,000 offered updates per case; 1 connection at 200/250 requests/s and 4 at 400/800 requests/s; three runs per condition, tracing off/on order reversed on alternate repetitions. Both variants use the same `catdb-tail-profile:local` image and existing bounded application diagnostics. The only comparison variable is kernel tracing. Both keep 2-CPU / 4-GiB server and client limits, the 64-outstanding client budget and up-to-8 distinct-key batches. No artificial sync delay is injected in the main cohort. This measures warm-cache updates on the same Docker Desktop Linux guest, not native Windows storage.','',
'![Observer comparison](observer.svg)','',
'Bars are medians of three runs, whiskers are min/max. Response includes scheduled arrival, client queueing, TCP and server work; rejected requests are excluded from latency percentiles and reported separately. All results, including unfavorable changes, are retained.','',
'| Connections / offered rate | p99 off / on (median ms) | Largest response off / on (ms) | Completed/s off / on (median) | Rejections off / on |','|---|---:|---:|---:|---:|']
for c in cases:
 g=[[r for r in results if r['variant']==v and (r['clients'],r['rate'])==c] for v in variants]
 fmt=lambda vals:' / '.join(f'{x:.3f}' for x in vals)
 lines.append(f'| {c[0]} / {c[1]} | '+fmt([statistics.median(r['response']['p99_ms'] for r in a) for a in g])+' | '+fmt([max(r['response']['max_ms'] for r in a) for a in g])+' | '+fmt([statistics.median(r['ops_s'] for r in a) for a in g])+' | '+' / '.join(str(sum(r['rejected'] for r in a)) for a in g)+' |')
lines+=['','## What the trace establishes','',f"{len(all_sync):,} fsync calls matched the benchmark's synchronization counters exactly, with no missing wakeup/return pairs and zero reported per-CPU overrun, commit-overrun or dropped events. All twelve trace instances were stopped and removed. Text timestamps have microsecond precision; nanosecond fields represent that resolution, not nanosecond accuracy.",'',
'![Longest sync calls](sync-tail.svg)','',
f"The longest captured syscall took **{worst['duration_ns']/1e6:.3f} ms**: filesystem/I/O-path blocked **{worst['io_stack_blocked_ns']/1e6:.3f} ms**, other/unclassified blocked **{worst['other_blocked_ns']/1e6:.3f} ms**, runnable-to-scheduled **{worst['runnable_ns']/1e6:.3f} ms**, scheduled **{worst['scheduled_ns']/1e6:.3f} ms** (`{worst['case']}`, guest TID {worst['tid']}). These components belong to the same syscall; they are not independently selected maxima.",'',
f"In that longest call, the journal-commit wait was {sum(x['end_ns']-x['start_ns'] for x in worst['stacks'] if 'jbd2_log_wait_commit' in x['stack'])/1e6:.3f} ms. Across all {len(all_sync):,} observed calls, the largest total runnable delay within one sync was {max(x['runnable_ns'] for x in all_sync)/1e6:.3f} ms. See [the exact finding and stacks](findings.json). These values characterize this cohort, not an upper bound for future scheduling delays.",'',
'A blocked interval is classified as filesystem/I/O-path only when its recorded stack contains `io_schedule`, `jbd2_log_wait_commit`, or the observed writeback wait helpers. A D-state alone is insufficient: the pilot also recorded D-state memory-map locks. Wakeup-to-switch-in and runnable preemption intervals are counted separately. A missing wakeup invalidates the attribution.','',
'The I/O-path category is **not physical disk service time**. Journal commit waits include work and scheduling of journal/writeback workers; the trace does not separate their runnable delays, host Windows scheduling, virtualization or device firmware. Guest block issue/complete events are retained as context only: shared journal traffic, flushes and partial completions cannot be assigned to this WAL by temporal overlap alone. Scheduled time includes interrupts and possible host-vCPU descheduling.','',
'Tracefs uses `mono`; the client uses the same guest monotonic clock. Only fsync entries inside the offered workload window are selected; returns and scheduler transitions are followed to completion. Kernel TIDs pair syscall/scheduler events directly. Exact sync-count agreement checks coverage, but this does not establish a one-to-one identity between a group sync and an individual network update.','',
'## Decision and limits','',
'No durability-relaxing change, priority boost, filesystem option or new batching delay was adopted. The captured stacks expose filesystem/journal waits, not a demonstrated application-lock fix. Prior append `fdatasync` and preallocation trials already failed to remove the sync floor; repeating them without a new hypothesis would not justify a default change. See the [earlier synchronization study](../log-sync-2026-11-05/README.md). Maximum-wait reduction remains unachieved.','',
'The final collector adds a bounded 16-MiB-per-CPU ring (512 MiB on this 32-CPU guest), blocked-stack recording, global fsync/block tracepoints and target-name-filtered scheduler events. It polls its stop file every 100 ms. Trace dumping/compression happens after the workload. This costs memory and execution time even if a particular latency percentile improves by chance. Three repetitions on one shared workstation do not isolate every source of host variance.','',
'## Safety and reproducibility','',
f'{total:,} requests: {done:,} committed, {rejected:,} client admission rejections, zero operation failures. {checks} full-content checks passed, including six service restarts. Timed cohorts did not overlap builds, test suites or chart rendering. Small host-side source/document edits and occasional reads of completed data occurred; other host activity was not fully controlled. Production files and PALV libraries were not used.','',
'The production engine and service source were not modified. Existing safety tests and new attribution tests are recorded in [validation.txt](validation.txt); process-crash coverage does not prove real power-loss durability.','',
'Use [the harness](../../../benchmarks/sync-os/README.md), [all numerical results](results.json), [environment](environment.json), [source/image provenance](provenance.json), [exact syscall partitions](kernel-analysis.json.gz), and each `candidate-profile-trace-*` directory for raw kernel traces and loss counters. The failed first pilot (collector name reuse and initially missing handler-thread filter) was corrected before the successful pilot and is excluded from the main results.','',
'## Initial bounded-buffer attempt — retained, not silently discarded','',
'The first complete 24-case cohort used 4 MiB per CPU. All workload checks passed, but five of twelve traced cases reported overwritten events (all single-connection cases). Their syscall attribution was rejected. The full cohort is retained in [initial-attempt/results.json](initial-attempt/results.json), with [invalid-case names](initial-attempt/loss-invalid-cases.json) and raw traces/loss counters. Its unfavorable latency results remain available; they are not pooled with the final 16-MiB-per-CPU comparison. Seven complete initial traces remain valid evidence, including a 70.099-ms sync with 70.043-ms I/O-path blocking and only 0.011-ms runnable delay. This is a different cohort from the figures above.','',
'A single bounded retry repeated both off/on variants at the larger capacity. The export-failure path was also hardened between cohorts: injected trace-export failure now still disables events and removes the owned instance. This changes observer cleanup, not the DB or its successful commit path. Both measured-source archives preserve their respective collector versions.','',
'The collector uses a unique trace instance, leaves global tracing settings untouched, and disables events/removes the instance after capture. Linux documents the [trace clocks, buffers and instances](https://docs.kernel.org/trace/ftrace.html) and [event filters and triggers](https://docs.kernel.org/trace/events.html).', '']
(dest/'README.md').write_text('\n'.join(lines),encoding='utf-8')
readme=root/'README.md';text=readme.read_text();heading='## Follow-up: separating log-sync I/O-path and runnable waits'
already_present=heading in text
text+='\n'+heading+'\n\nThe same server binary was measured with kernel tracing off/on, at 100,000 rows and 1/4 connections. This identifies waits inside sync and the observer effect; it does **not** establish a maximum-latency improvement.\n\n![Kernel sync wait attribution](docs/benchmarks/sync-os-2026-11-05/sync-tail.svg)\n\n![Kernel tracing observer effect](docs/benchmarks/sync-os-2026-11-05/observer.svg)\n\nSee the [full results, safety checks, raw traces and limitations](docs/benchmarks/sync-os-2026-11-05/README.md). Filesystem/journal blocking is distinguished from runnable-to-scheduled delay; guest observations do not identify physical-device latency. The commit policy and durability guarantees are unchanged.\n'
if not already_present:readme.write_text(text,encoding='utf-8')
print(json.dumps(dict(cases=len(results),syncs=len(all_sync),worst={k:v for k,v in worst.items() if k!='stacks'},requests=total,success=done,rejected=rejected,checks=checks),indent=2))
