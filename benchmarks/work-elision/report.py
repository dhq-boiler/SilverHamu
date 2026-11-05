from pathlib import Path
import csv,gzip,json,math,statistics,sys
root=Path(sys.argv[1]);out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=False)
summary=[]
for folder in sorted(p for p in root.iterdir() if p.is_dir()):
 size,repeat,variant,mode=folder.name.split('-');raw=(folder/'samples.csv').read_bytes();(out/(folder.name+'.csv.gz')).write_bytes(gzip.compress(raw,mtime=0))
 groups={}
 for row in csv.DictReader(raw.decode().splitlines()):
  assert row['counted']==str(mode=='alloc').lower()
  if mode=='time':assert row['calls']==row['bytes']=='0'
  groups.setdefault((row['case'],int(row['dependencies'])),[]).append(row)
 for (case,deps),rows in groups.items():
  times=sorted(int(r['ns']) for r in rows)
  summary.append(dict(size=size,repeat=int(repeat),variant=variant,mode=mode,case=case,dependencies=deps,count=len(rows),p50_ns=statistics.median(times),p99_ns=times[math.ceil(.99*len(times))-1],max_ns=max(times),calls=statistics.median(int(r['calls']) for r in rows),bytes=statistics.median(int(r['bytes']) for r in rows)))
assert len(list(out.glob('*.csv.gz')))==36
(out/'summary.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
def get(size,variant,mode,case,deps,field):
 a=[r[field] for r in summary if (r['size'],r['variant'],r['mode'],r['case'],r['dependencies'])==(str(size),variant,mode,case,deps)];assert len(a)==3;return statistics.median(a)
lines=['| Dataset | Operation | Dependencies / value bytes | Before p99 µs | After p99 µs | Before alloc calls | After alloc calls | Before allocated bytes | After allocated bytes |','|---|---|---:|---:|---:|---:|---:|---:|---:|']
for size in ['100000','1000000','service']:
 cases=[(c,d) for c in ['validate','commit'] for d in [1,100,10000]]+[('export',0)] if size!='service' else [(c,d) for c in ['put_commit','get_latest'] for d in [8,262144]]
 for case,deps in cases:
  values=[get(size,v,'time',case,deps,'p99_ns')/1000 for v in ['before','after']]+[get(size,v,'alloc',case,deps,'calls') for v in ['before','after']]+[get(size,v,'alloc',case,deps,'bytes') for v in ['before','after']]
  lines.append(f'| {size} | {case} | {deps} | '+' | '.join(f'{x:.3f}' for x in values)+' |')
lines+=['','Timing columns: median of three trial p99s, except export (one operation per trial; median elapsed time). Allocation columns: median of the per-trial median operation count/bytes, from separate instrumented builds. Values are cumulative allocated/requested bytes, not live or peak memory.']
(out/'table.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
fig,axs=plt.subplots(2,2,figsize=(12,8),constrained_layout=True)
for ax,case,scale,title in [(axs[0,0],'validate',1000,'1M rows: point validation p99 (µs)'),(axs[0,1],'commit',1e6,'1M rows: durable commit p99 (ms)')]:
 for j,v in enumerate(['before','after']):ax.bar([i+(j-.5)*.32 for i in range(3)],[max(.05,get('1000000',v,'time',case,d,'p99_ns')/scale) for d in [1,100,10000]],width=.3,label=v,color=['#64748b','#007f87'][j])
 ax.set_xticks(range(3),['1 key','100 keys','10,000 keys']);ax.set_ylabel(title)
 if case=='validate':ax.set_yscale('log')
ax=axs[1,0]
for j,v in enumerate(['before','after']):ax.bar([i+(j-.5)*.32 for i in range(2)],[get('service',v,'alloc','put_commit',d,'bytes')/1024 for d in [8,262144]],width=.3,color=['#64748b','#007f87'][j])
ax.set_xticks(range(2),['8-byte value','256-KiB value']);ax.set_ylabel('Single-request put: allocated KiB (log)');ax.set_yscale('log')
ax=axs[1,1]
for j,v in enumerate(['before','after']):ax.bar([i+(j-.5)*.32 for i in range(2)],[get(n,v,'time','export',0,'p99_ns')/1e6 for n in ['100000','1000000']],width=.3,color=['#64748b','#007f87'][j])
ax.set_xticks(range(2),['100k rows','1M rows']);ax.set_ylabel('Export elapsed time (ms)')
for ax in axs.flat:ax.set_axisbelow(True);ax.grid(axis='y',alpha=.2)
axs[0,0].legend();fig.suptitle('Removing redundant work: matched before/after, three trials\nProduction engine / in-process service dispatch; one writer; not TCP throughput',fontsize=13)
fig.supxlabel('Timing uses counters compiled out. Allocation counts use separate instrumented builds; cumulative bytes are not RSS.',fontsize=10)
fig.savefig(out/'comparison.svg');fig.savefig(out/'comparison.png',dpi=160)
print('\n'.join(lines))
