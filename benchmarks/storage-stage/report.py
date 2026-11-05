from pathlib import Path
import json,gzip,shutil,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parents[2];raw=Path(sys.argv[1]);dest=root/'docs/benchmarks/storage-stage-2026-11-06';dest.mkdir(exist_ok=True)
rows=json.loads((raw/'results.json').read_text());summary=json.loads((raw/'storage-summary.json').read_text());valid=[s for s in summary if s['valid']];invalid=[s for s in summary if not s['valid']]
assert len(rows)==12 and all(r['completed']==8000 and not r['errors'] and not r['rejected'] for r in rows)
fig,axes=plt.subplots(1,2,figsize=(10,4),layout='constrained');table=[]
for i,c in enumerate([1,4]):
 for j,on in enumerate([False,True]):
  rs=[r for r in rows if r['clients']==c and ('trace' in r['variant'])==on]
  table.append((c,on,statistics.median(r['response']['p99_ms'] for r in rs),max(r['response']['max_ms'] for r in rs),statistics.median(r['ops_s'] for r in rs)))
  for ax,field in zip(axes,['p99_ms','max_ms']):
   vs=[r['response'][field] for r in rs];m=statistics.median(vs);x=i+(-.18 if not on else .18)
   ax.bar(x,m,width=.32,color='#d78932' if on else '#227b8e',label=('Trace on' if on else 'Trace off') if i==0 else None);ax.errorbar(x,m,yerr=[[m-min(vs)],[max(vs)-m]],fmt='none',color='black',capsize=4)
for ax,title in zip(axes,['Response p99','Per-run maximum response']):
 ax.set_title(title);ax.set_ylabel('Milliseconds');ax.set_xticks([0,1],['1 connection / 200/s','4 connections / 800/s']);ax.legend();ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
fig.suptitle('Virtual storage tracing: unchanged engine, 100k rows, 8k updates\nThree runs; bars = medians, whiskers = min–max',fontsize=11)
fig.savefig(dest/'observer-cost.svg');fig.savefig(raw/'observer-cost-preview.png',dpi=120);plt.close(fig)
fig,ax=plt.subplots(figsize=(10,5),layout='constrained');left=[0]*len(valid)
for key,label,color in [('pre_driver_ns','Before driver entry','#357b8f'),('driver_to_done_ns','Driver entry → completion notification (includes unknown host interval)','#d78932'),('after_done_ns','Notification → flush callback','#64914f')]:
 vs=[s['worst'][key]/1e6 for s in valid];ax.barh(range(len(valid)),vs,left=left,label=label,color=color);left=[a+b for a,b in zip(left,vs)]
ax.set_yticks(range(len(valid)),[s['case'].replace('candidate-profile-trace-','').replace('-normal','') for s in valid]);ax.set_xlabel('Milliseconds');ax.set_title('Longest matched flush in each accepted trace');ax.legend(fontsize=8,loc='upper center',bbox_to_anchor=(.5,-.17));ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True)
fig.savefig(dest/'flush-stages.svg');fig.savefig(raw/'flush-stages-preview.png',dpi=120);plt.close(fig)
for p in dest.glob('*.svg'):p.write_text('\n'.join(l.rstrip() for l in p.read_text().splitlines())+'\n')
for p in raw.iterdir():
 if p.is_file():shutil.copy2(p,dest/p.name)
 elif (p/'metadata.json').exists():shutil.copytree(p,dest/p.name,dirs_exist_ok=True)
shutil.copytree(raw.parent/'storage-stage-pilot',dest/'pilot',dirs_exist_ok=True)
shutil.copy2(raw.parent/'storage-stage-export-failure/result.json',dest/'export-failure-validation.json')
validation=[]
for p in (raw.parent/'storage-stage-validation').glob('*.txt'):
 s=p.read_text(encoding='utf-8-sig',errors='replace');validation.extend([p.name]+[l for l in s.splitlines() if 'test result:' in l or l.startswith('Ran ') or l=='OK'])
(dest/'validation.txt').write_text('\n'.join(validation)+'\n')
lines=['# Virtual storage completion stages — 2026-11-06','',
'**This is an observer study, not a DB speedup.** The production engine, durability settings and defaults remain unchanged. The driver-side request can be followed across submission and completion; the host-internal interval remains opaque.','',
'## Method and observer cost','',
'100,000 synthetic records × 256 bytes, 1 connection at 200 offered requests/s and 4 at 800/s; 8,000 updates per case, three off/on repetitions with alternating order. Both variants use the same profile-enabled image. Fixed arrival rates measure response latency, not peak throughput. All 96,000 main updates committed, with no rejections or operation errors. Full-content checks follow each condition and restart. Preliminary 1,000-update cases are retained separately.','',
'![Observer cost](observer-cost.svg)','',
'| Connections | Tracing | Median run p99 ms | Maximum across runs ms | Median updates/s |','|---:|---|---:|---:|---:|']
for c,on,p,m,o in table:lines.append(f'| {c} | {"on" if on else "off"} | {p:.3f} | {m:.3f} | {o:.1f} |')
lines+=['','## Same-request decomposition','',
'The running WSL kernel BTF supplies the request size (240 bytes here); SCSI command pointers are converted using the [kernel PDU layout](https://raw.githubusercontent.com/torvalds/linux/v6.18/include/linux/blk-mq.h) and [SCSI conversion](https://raw.githubusercontent.com/torvalds/linux/v6.18/include/scsi/scsi_cmnd.h), then anonymized. Request dispatch serials distinguish reuse. The [Hyper-V storage driver](https://raw.githubusercontent.com/torvalds/linux/v6.18/drivers/scsi/storvsc_drv.c) is observed at queuecommand entry/return and scsi_done within channel callback context. This avoids nearest-time ownership assumptions.','',
'Driver entry precedes the actual transport send; scsi_done follows receive processing. The middle span therefore includes preparation, transport, host/storage work, virtual CPU/interrupt delivery and receive work. It must not be relabeled physical disk time. A channel callback can service multiple completions; it is not a one-request ID.','',
'![Flush stages](flush-stages.svg)','',
'| Accepted case | Matched flushes | Longest total ms | Before driver ms | Driver → notification ms | After notification ms |','|---|---:|---:|---:|---:|---:|']
for s in valid:
 w=s['worst'];lines.append(f"| {s['case']} | {s['flushes']} | {w['total_ns']/1e6:.3f} | {w['pre_driver_ns']/1e6:.3f} | {w['driver_to_done_ns']/1e6:.3f} | {w['after_done_ns']/1e6:.3f} |")
lines+=['',f"Accepted captures: {len(valid)}/6. Invalid captures: "+('; '.join(s['case']+': '+', '.join(s['issues']) for s in invalid) or 'none')+'. Invalid raw data are retained and excluded from causal interpretation; their client timing remains in the observer-cost comparison.','',
'Each row splits the same longest flush, so its components can be compared directly. Different rows and the response maxima refer to different requests. Channel-entry timestamps, submission-return status and parent journal identities remain in the raw analysis. Submission return can occur after completion on a fast path and is not blindly subtracted as an earlier timestamp.','',
'In this run, 36,336 flushes were matched in three accepted captures. The longest was 11.677 ms: 0.001 ms before driver entry, 11.668 ms until completion notification, and 0.008 ms afterward. Three of six traces had missed dynamic-probe instances (including completion/channel probes); complete three-repetition causal validation was not achieved. No extended or selective rerun was performed. The observer needs a more reliable event path before further fine-grained causal claims.','',
'## Limits, safety and reproduction','',
'No host ETW/physical-device correlation was recorded. The study cannot distinguish host queueing, physical storage latency and virtual interrupt/vCPU delivery inside the middle interval. Earlier 129-ms and 1.47-s events are not retrospectively explained. No application optimization or weaker synchronization is justified by this result alone. Variation and any negative observer effect are preserved.','',
'Fifteen owned dynamic probes, a private trace instance, a 16 MiB/CPU ring (512 MiB capacity here), and a 180-second deadline bound capture. Missing probes, ring loss, unmatched stages and invalid ordering reject attribution. Pointer conversion uses validated BTF, not a guessed layout; raw pointers are anonymized before publication. Export-failure cleanup is tested, although forced process/VM termination can bypass cleanup.','',
'Tests cover request reuse, retries, failure status, missing/duplicate events, CPU context isolation, shared receive callbacks, completion-before-return and malformed/ambiguous BTF. Existing Rust/service/C ABI tests retain snapshot, conflict and process-crash checks. They do not establish physical power-loss durability. Tests, builds and chart rendering ran after timing; small host source/document edits and reads occurred during measurement, and unrelated host/device activity was not fully controlled.','',
'See [reproduction](../../../benchmarks/storage-stage/README.md), [validation](validation.txt), [results](results.json), [stage summary](storage-summary.json), [exact matched requests](storage-analysis.json.gz), [provenance](provenance.json) and [pilot](pilot/results.json).','']
(dest/'README.md').write_text('\n'.join(lines),encoding='utf-8')
p=root/'README.md';s=p.read_text(encoding='utf-8');heading='## Follow-up: locating the virtual storage completion interval';already_present=heading in s
if not already_present:p.write_text(s+'\n'+heading+'\n\nThe same flush is now followed from block dispatch through virtual-storage driver submission and completion notification. Host-internal time remains unresolved; no speedup or durability change is claimed.\n\n![Virtual storage stages](docs/benchmarks/storage-stage-2026-11-06/flush-stages.svg)\n\nSee [observer overhead, matched requests, failed captures and limits](docs/benchmarks/storage-stage-2026-11-06/README.md).\n',encoding='utf-8')

