import csv,gzip,json,math,pathlib,statistics,sys
root=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2]);out.mkdir(exist_ok=False,parents=True)
summary=[]
for folder in sorted(p for p in root.iterdir() if p.is_dir()):
 n,shape,repeat,mode=folder.name.split('-');raw=(folder/'samples.csv').read_bytes()
 (out/(folder.name+'.csv.gz')).write_bytes(gzip.compress(raw,mtime=0))
 groups={}
 for row in csv.DictReader(raw.decode().splitlines()):groups.setdefault(row['case'],[]).append(row)
 for case,rows in groups.items():
  times=sorted(int(r['ns']) for r in rows)
  summary.append(dict(rows=int(n),shape=shape,repeat=int(repeat),mode=mode,case=case,count=len(times),p50_ns=statistics.median(times),p99_ns=times[math.ceil(len(times)*.99)-1],max_ns=max(times),conflicts=sum(r['conflict']=='true' for r in rows)))
assert len(list(out.glob('*.csv.gz')))==24
(out/'summary.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
def get(n,shape,mode,case,field='p99_ns'):
 a=[r[field] for r in summary if (r['rows'],r['shape'],r['mode'],r['case'])==(n,shape,mode,case)]
 assert len(a)==3
 return statistics.median(a)
lines=['| Rows | Updates | Case | Before p99 µs | After p99 µs | Before max µs¹ | After max µs¹ |','|---:|---|---|---:|---:|---:|---:|']
for n in [100000,1000000]:
 for shape in ['local','spread']:
  for case in ['stable_wide','stable_narrow','stale_upper_half','stale_narrow','prepare','commit']:
   tails=[get(n,shape,m,case)/1000 for m in ['before','after']]
   maxima=[max(r['max_ns'] for r in summary if (r['rows'],r['shape'],r['mode'],r['case'])==(n,shape,m,case))/1000 for m in ['before','after']]
   lines.append(f'| {n:,} | {shape} | {case} | '+' | '.join(f'{v:.3f}' for v in tails+maxima)+' |')
lines+=['','p99 columns are medians of three trial p99s. ¹Max columns are maxima across all three trials; they are not medians.']
(out/'table.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
fig,axs=plt.subplots(2,2,figsize=(12,8),constrained_layout=True)
conditions=[(100000,'local'),(100000,'spread'),(1000000,'local'),(1000000,'spread')]
for ax,case,label,scale,log in [(axs[0,0],'stable_wide','Unchanged generation:\nfull-range validation p99 (µs)',1000,True),(axs[0,1],'stale_upper_half','Advanced generation:\nupper-half validation p99 (µs)',1000,True),(axs[1,0],'commit','Durable commit after range read p99 (ms)',1e6,False),(axs[1,1],'prepare','Prepare: range page + staged put p99 (µs)',1000,False)]:
 for j,m in enumerate(['before','after']):
  ax.bar([i+(j-.5)*.32 for i in range(4)],[max(.1 if log else 0,get(n,s,m,case)/scale) for n,s in conditions],width=.3,color=['#64748b','#007f87'][j],label=['Before','Generation guard'][j])
 ax.set_xticks(range(4),['100k\nlocal','100k\nspread','1M\nlocal','1M\nspread']);ax.set_ylabel(label,fontsize=10);ax.set_axisbelow(True);ax.grid(axis='y',alpha=.2)
 if log:ax.set_yscale('log')
axs[0,0].legend()
fig.suptitle('Production engine: skip committed-state range scans only when generation is unchanged\nMedian of three trial p99s · one writer · u64 payload · durable append WAL',fontsize=13)
fig.supxlabel('Validation excludes lock acquisition. Range samples n=96/trial; commit samples n=192/trial. Full raw data and all-trial maxima are published.',fontsize=9)
fig.savefig(out/'comparison.svg');fig.savefig(out/'comparison.png',dpi=160)
print('\n'.join(lines))
