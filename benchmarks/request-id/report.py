from pathlib import Path
import json,gzip,shutil,collections,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parents[2];raw=Path(sys.argv[1]);dest=root/'docs/benchmarks/request-id-2026-11-06';dest.mkdir(exist_ok=True);bench=root/'benchmarks/request-id'
rows=json.loads((raw/'results.json').read_text());summary=json.loads((raw/'summary.json').read_text())
with gzip.open(raw/'request-analysis.json.gz','rt') as f:analysis=json.load(f)
assert len(rows)==4 and all(r['completed']==1000 and not r['errors'] and not r['rejected'] for r in rows)
categories=['Journal-thread context','DB thread, journal unknown','Worker, journal unknown','Other / unknown'];counts=[]
for a in analysis:
 c=collections.Counter()
 for x in a['attempts']:
  c[categories[0] if x['journal_context'] else categories[1] if x['issuer']=='catdb-client' else categories[2] if x['issuer'].startswith('kworker/') else categories[3]]+=1
 counts.append(c)
fig,ax=plt.subplots(figsize=(10,4),layout='constrained');left=[0,0]
for category,color in zip(categories,['#237b8e','#67a976','#e78b29','#999999']):
 values=[c[category] for c in counts];ax.barh(range(2),values,left=left,label=category,color=color);left=[a+b for a,b in zip(left,values)]
ax.set_yticks(range(2),['4 connections / 800 requests/s','1 connection / 200 requests/s']);ax.set_xlabel('Guest-wide dispatch attempts during each 1,000-update workload');ax.set_title('Request identity observed; worker-to-journal parentage remains unresolved');ax.legend(fontsize=8,loc='lower right');ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True);fig.savefig(dest/'request-context.svg');fig.savefig(raw/'request-context-preview.png',dpi=120);plt.close(fig)
p=dest/'request-context.svg';p.write_text('\n'.join(l.rstrip() for l in p.read_text().splitlines())+'\n')
for p in raw.iterdir():
 if p.is_file():shutil.copy2(p,dest/p.name)
 elif (p/'metadata.json').exists():shutil.copytree(p,dest/p.name,dirs_exist_ok=True)
for name in ['request-id-pilot','request-id-pilot-v2','request-id-pilot-v3']:shutil.copytree(raw.parent/name,dest/name,dirs_exist_ok=True)
shutil.copy2(raw.parent/'request-id-export-failure/result.json',dest/'export-failure-validation.json')
text='Validation: unchanged production engine and request observer\n\n'
for name in ['analysis','rust','protocol','previous-analysis','ffi']:
 s=(raw.parent/'request-id-validation'/f'{name}.txt').read_text(encoding='utf-8-sig',errors='replace');text+=name+'\n'+'\n'.join(l for l in s.splitlines() if 'test result:' in l or l.startswith('Ran ') or l=='OK')+'\n\n'
text+='Export failure: instance removed; separate read-only inspection found no remaining trace instances or dynamic probes.\n'
(dest/'validation.txt').write_text(text)
lines=['# I/O request identity feasibility — 2026-11-06','',
'**Progress: dispatch attempts can now be followed by request identity, but worker-to-journal parentage is still missing.** The full 8,000-request / three-repetition comparison was not run because both identity and journal attribution were prerequisites. No maximum-latency improvement or production-policy change is claimed.','',
'## Method','',
'The unchanged server and client use 100,000 synthetic records × 256 bytes, 1 connection at 200 offered requests/s and 4 at 800/s. The final pilot has one off/on run per condition and only 1,000 updates each. A private trace instance captures existing scheduler/journal/block events plus six owned dynamic probes: request start, end, update entry/return, free and requeue. Kernel request addresses are replaced with opaque aliases before export; the aliases are per-capture memory slots, not permanent request or database transaction IDs. Each new dispatch gets a sequence number.','',
'![Request context](request-context.svg)','',
'| Condition | Dispatch attempts | Observed data completion | Direct journal-thread context | Journal context unknown | No end-function hook |','|---|---:|---:|---:|---:|---:|']
for s in summary:lines.append(f"| {s['case']} | {s['dispatches']} | {s['data_done']} | {s['direct_journal_context']} | {s['unassigned_journal_context']} | {s['without_end_hook']} |")
lines+=['',
'All 6,422 observed dispatch attempts in the final workload windows reached an observed data-completion return. The captures reported zero ring overruns/drops and zero missed dynamic-probe instances. The 1,283 fsync calls agreed with the service counters. These are **guest-wide observations during the workload**, not proof that every request belongs to this database. Direct journal-thread context was observed for 1,283 dispatches; the other 5,139 include DB-thread writes, worker work and possibly unrelated I/O.','',
'Repeated use of the same request slot occurred. Moreover, 5,139 completed dispatches did not pass through the ordinary request-end hook. Following only that hook or treating the opaque slot as a permanent ID would miss completions or merge distinct attempts. The update-return value distinguishes whether data remain; data completion is not equivalent to logical-request release, flush completion, database commit or durability. The analyzer retains free/requeue events separately. No real partial update or requeue occurred in this short pilot; those branches were checked with synthetic event sequences, not claimed as live fault-injection coverage.','',
'## What still prevents attribution','',
'Some requests are dispatched by a worker rather than the journal thread. This observer has no proven parent/child mapping from an originating journal transaction through the flush queue, worker handoff, request merging/splitting and completion. Temporal overlap is deliberately not used to invent that relationship. The next required evidence is that handoff mapping, not another long run of the same observer. Native Windows/device attribution also remains outside this guest trace. The previous 129-ms event is not explained retrospectively.','',
'The [kernel tracing interface](https://docs.kernel.org/trace/kprobetrace.html) supports capturing function arguments and return values. The interpretation of request update/end paths was checked against [the Linux block request implementation](https://raw.githubusercontent.com/torvalds/linux/v6.18/block/blk-mq.c). These probes are kernel-version-specific diagnostics, not a portable DB API. The collector uses SYS_ADMIN, owned dynamic event names and a private trace instance; it does not truncate the global probe registry or change storage policy.','',
'## Preliminary observer results','',
'| Connections / rate | Trace | p99 response ms | Maximum response ms | Updates/s |','|---|---|---:|---:|---:|']
for r in rows:lines.append(f"| {r['clients']} / {r['rate']} | {'on' if 'trace' in r['variant'] else 'off'} | {r['response']['p99_ms']:.3f} | {r['response']['max_ms']:.3f} | {r['ops_s']:.1f} |")
lines+=['',
'One short repetition cannot establish tracing overhead or a performance improvement. The bounded ring still reserves up to 16 MiB per CPU (512 MiB on this guest). No durability policy, service source or Rust engine implementation changed. All 4,000 updates committed, with zero rejection/operation error; eight full-content checks including two restarts passed. The safety suites ran after timing completed.','',
'## Retained development attempts and safety','',
'`request-id-pilot` failed observer registration: Python append-mode file initialization attempted a seek unsupported by tracefs. Raw append writes fixed it without truncating existing probes. `request-id-pilot-v2` captured only the five entry probes and therefore lacked the update return. `request-id-pilot-v3` added returns but failed to retain probe-miss statistics because this kernel reports event names without the group prefix. These attempts are retained separately and are not pooled with the final results. A malformed return-expression construction was also caught by a registration-only check before workloads.','',
'The final observer rejects conflicting preexisting event names, bounds capture duration, records probe-miss counters, and removes its event registrations after disabling/removing its instance. Export failure was injected successfully and a separate inspection confirmed no remaining probes or instances. Forced process/VM termination can still prevent cleanup; this is not a production monitoring service.','',
'Existing engine tests cover snapshots, conflicts, persistence failure and process-crash recovery; new event tests cover reuse, partial updates, requeue, preexisting requests, unmatched entry/return, invalid returns and refusing temporal journal assignment. See [validation.txt](validation.txt). These tests do not prove physical power-loss durability or every real I/O error path.','',
'See [reproduction](../../../benchmarks/request-id/README.md), [results](results.json), [summary](summary.json), [exact attempts](request-analysis.json.gz), [source/image provenance](provenance.json), and the trace folders for evidence. No raw kernel addresses are published in the exported request fields.','']
(dest/'README.md').write_text('\n'.join(lines))
(bench/'README.md').write_text('''# Request identity pilot

This is a kernel-version-specific feasibility observer, not a DB optimization. It follows opaque request slots plus dispatch sequence numbers, including update-return state. Worker-to-journal parentage is still unresolved, so long performance runs did not pass the prerequisite.

Use the existing `catdb-tail-profile:local` image (`service,backlog-batch,benchmark-metrics,sync-stall,lock-timing`) and comparison base image:

```powershell
python benchmarks/request-id/run.py --variant candidate-profile-off=catdb-tail-profile:local --variant candidate-profile-trace=catdb-tail-profile:local --out C:/scratch/request-pilot --reps 1 --ops 1000 --cases 1:200 4:800 --scenarios normal
python benchmarks/request-id/analyze.py C:/scratch/request-pilot
python benchmarks/request-id/test_analysis.py
python benchmarks/request-id/check_export_failure.py C:/scratch/request-export-failure
```

Output directories must be new. Data use disposable Docker volumes. The collector requires SYS_ADMIN and tracefs dynamic-probe support, and owns only its unique probe group and instance. It refuses preexisting colliding probe names because this kernel's miss statistics omit group names. Do not run other catdb services/probe collectors during capture. The ring is bounded to 16 MiB per CPU, capture to 180 seconds; dump and alias replacement occur after tracing stops. A forced kill may leave owned resources: inspect the instance/registered events and remove only those owned by this capture.

The analyzer rejects loss or incomplete entry/return pairing. A slot is a memory address alias, not an allocation-generation ID; dispatch counters prevent merging repeated dispatches. `more=0` means no data remain at that update, not that a journal or DB commit is durable. Final release, retries, worker ownership and logical request lifetime are separate concepts. Context from the current journal thread is retained without assigning worker requests by time proximity. Partial/requeue paths have synthetic tests; this pilot did not exercise them live.
''')
p=root/'README.md';s=p.read_text();heading='## Follow-up: identifying I/O dispatches without inventing journal ownership';assert heading not in s
p.write_text(s+'\n'+heading+'\n\nA short observer pilot followed 6,422 dispatch attempts to data completion using opaque request identities and per-slot dispatch counters. Worker-to-journal parentage remains unresolved, so the long comparison was not started and no speedup is claimed.\n\n![I/O request context](docs/benchmarks/request-id-2026-11-06/request-context.svg)\n\nSee [the identity evidence, limitations, observer results and retained attempts](docs/benchmarks/request-id-2026-11-06/README.md).\n')
