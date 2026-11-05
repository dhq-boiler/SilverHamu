"""Summarize paired full scans without treating eight samples as a tail estimate."""
import csv,gzip,json,statistics,sys
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(sys.argv[1]);out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
trials=[]
for path in sorted(root.glob('*/samples.csv')):
    size,shape,repeat,variant,mode=path.parent.name.split('-')
    rows=list(csv.DictReader(path.open(encoding='utf-8')))
    assert len(rows)==8
    assert all(r['counted']==str(mode=='alloc').lower() for r in rows)
    item=dict(size=int(size),shape=shape,repeat=int(repeat),variant=variant,mode=mode)
    for field in ['ns','calls','bytes','buffer_current','buffer_peak','rows']:
        values=[int(r[field]) for r in rows]
        item[field]=dict(median=statistics.median(values),minimum=min(values),maximum=max(values))
    trials.append(item)
    with gzip.open(out/(path.parent.name+'.csv.gz'),'wb') as f:f.write(path.read_bytes())
assert len(trials)==48
groups=[]
for size in [100000,1000000]:
 for shape in ['plain','mixed']:
  for variant in ['before','after']:
   group=dict(size=size,shape=shape,variant=variant)
   for field,mode in [('ns','time'),('calls','alloc'),('bytes','alloc'),('buffer_current','alloc'),('buffer_peak','alloc')]:
    matched=[t[field] for t in trials if (t['size'],t['shape'],t['variant'],t['mode'])==(size,shape,variant,mode)]
    assert len(matched)==3
    medians=[t['median'] for t in matched]
    group[field]=dict(median=statistics.median(medians),trial_min=min(medians),trial_max=max(medians),all_max=max(t['maximum'] for t in matched))
   groups.append(group)
(out/'summary.json').write_text(json.dumps(dict(trials=trials,groups=groups),indent=2)+'\n',encoding='utf-8')
lines=['| Rows / shape | Version | Read median ms | All-scan maximum ms | Allocation calls | Requested MiB | Reserved MiB |','|---|---|---:|---:|---:|---:|---:|']
for g in groups:
 lines.append(f"| {g['size']:,} / {g['shape']} | {g['variant']} | {g['ns']['median']/1e6:.3f} | {g['ns']['all_max']/1e6:.3f} | {g['calls']['median']:,.0f} | {g['bytes']['median']/2**20:.2f} | {g['buffer_current']['median']/2**20:.2f} |")
(out/'table.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
fig,axes=plt.subplots(2,2,figsize=(12,8),layout='constrained')
labels=['100k\nplain','100k\nmixed','1M\nplain','1M\nmixed']
for ax,(field,scale,title) in zip(axes.flat,[('ns',1e6,'Full read time (ms)'),('calls',1000,'Allocation calls (thousands)'),('bytes',2**20,'Cumulative requested allocation (MiB)'),('buffer_current',2**20,'Reserved buffer budget (MiB)')]):
 for j,(variant,color) in enumerate([('before','#667085'),('after','#007f85')]):
  data=[g[field] for g in groups if g['variant']==variant]
  values=[d['median']/scale for d in data]
  x=[i+(j-.5)*.37 for i in range(4)]
  ax.bar(x,values,.35,label=variant,color=color,yerr=[[v-d['trial_min']/scale for v,d in zip(values,data)],[d['trial_max']/scale-v for v,d in zip(values,data)]],capsize=3)
  for xx,v,d in zip(x,values,data):ax.text(xx,d['trial_max']/scale,f'{v:,.1f}',ha='center',va='bottom',fontsize=8)
 ax.set_xticks(range(4),labels);ax.set_title(title);ax.set_ylim(0,ax.get_ylim()[1]*1.15);ax.spines[['top','right']].set_visible(False);ax.grid(axis='y',alpha=.15);ax.set_axisbelow(True)
axes[0,0].legend()
fig.suptitle('catdb: remove the intermediate full-scan tree',fontsize=16)
fig.supxlabel('Median of three trial medians; whiskers: trial range. Time and allocation builds are separate.\n8-byte keys / values; 8 scans per trial. Allocation is not peak memory or RSS.',fontsize=10)
fig.savefig(out/'comparison.svg');fig.savefig(out/'comparison.png',dpi=130)
print('\n'.join(lines))
