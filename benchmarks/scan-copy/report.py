import csv,gzip,json,statistics,sys
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(sys.argv[1]);out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
trials=[]
for path in sorted(root.glob('*/samples.csv')):
 size,shape,repeat,variant,mode=path.parent.name.split('-');raw=list(csv.DictReader(path.open(encoding='utf-8')))
 assert len(raw)==32
 assert all(r['counted']==str(mode=='alloc').lower() for r in raw)
 for operation in ['owned','framed','shared','export']:
  rows=[r for r in raw if r['operation']==operation];assert len(rows)==8
  item=dict(size=int(size),shape=shape,repeat=int(repeat),variant=variant,mode=mode,operation=operation)
  for field in ['ns','calls','bytes','buffer_current','buffer_peak']:
   values=[int(r[field]) for r in rows];item[field]=dict(median=statistics.median(values),minimum=min(values),maximum=max(values))
  trials.append(item)
 with gzip.open(out/(path.parent.name+'.csv.gz'),'wb') as f:f.write(path.read_bytes())
assert len(trials)==192
groups=[]
for operation in ['owned','framed','export','shared']:
 for size in [100000,1000000]:
  for shape in ['plain','mixed']:
   for variant in ['before','after']:
    group=dict(operation=operation,size=size,shape=shape,variant=variant)
    for field,mode in [('ns','time'),('calls','alloc'),('bytes','alloc'),('buffer_current','alloc'),('buffer_peak','alloc')]:
     matched=[t[field] for t in trials if (t['operation'],t['size'],t['shape'],t['variant'],t['mode'])==(operation,size,shape,variant,mode)]
     assert len(matched)==3
     medians=[t['median'] for t in matched]
     group[field]=dict(median=statistics.median(medians),trial_min=min(medians),trial_max=max(medians),all_max=max(t['maximum'] for t in matched))
    groups.append(group)
(out/'summary.json').write_text(json.dumps(dict(trials=trials,groups=groups),indent=2)+'\n',encoding='utf-8')
lines=['| Operation | Rows / shape | Version | Median ms | Maximum ms | Alloc calls | Requested MiB | Held budget MiB |','|---|---|---|---:|---:|---:|---:|---:|']
for g in groups:
 budget='released' if g['operation']=='export' else f"{g['buffer_current']['median']/2**20:.2f}"
 lines.append(f"| {g['operation']} | {g['size']:,} / {g['shape']} | {g['variant']} | {g['ns']['median']/1e6:.3f} | {g['ns']['all_max']/1e6:.3f} | {g['calls']['median']:,.0f} | {g['bytes']['median']/2**20:.2f} | {budget} |")
(out/'table.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
def plot(name,panels,title):
 fig,axes=plt.subplots(2,2,figsize=(12,8),layout='constrained')
 for ax,(operation,field,scale,label) in zip(axes.flat,panels):
  for j,(variant,color) in enumerate([('before','#667085'),('after','#007f85')]):
   data=[g[field] for g in groups if g['variant']==variant and g['operation']==operation];values=[d['median']/scale for d in data];x=[i+(j-.5)*.37 for i in range(4)]
   ax.bar(x,values,.35,label=variant,color=color,yerr=[[v-d['trial_min']/scale for v,d in zip(values,data)],[d['trial_max']/scale-v for v,d in zip(values,data)]],capsize=3)
   for xx,v,d in zip(x,values,data):ax.text(xx,d['trial_max']/scale,(f'{v:.3f}' if 0<v<0.1 else f'{v:,.1f}'),ha='center',va='bottom',fontsize=8)
  ax.set_xticks(range(4),['100k\nplain','100k\nmixed','1M\nplain','1M\nmixed']);ax.set_title(label);ax.set_ylim(0,ax.get_ylim()[1]*1.17);ax.spines[['top','right']].set_visible(False);ax.grid(axis='y',alpha=.15);ax.set_axisbelow(True)
 axes[0,0].legend(loc='upper left');fig.suptitle(title,fontsize=15)
 fig.supxlabel('Three trial medians; whiskers: trial range. Eight samples/trial. Separate time/allocation builds.\nAllocation requests are not peak memory or RSS. Export always reads committed rows.',fontsize=10)
 fig.savefig(out/(name+'.svg'));fig.savefig(out/(name+'.png'),dpi=130);plt.close(fig)
plot('comparison',[('owned','ns',1e6,'Unchanged owned scan (ms)'),('framed','ns',1e6,'Framed result construction (ms)'),('export','ns',1e6,'Durable file export (ms)'),('framed','bytes',2**20,'Framed construction: requested MiB')],'catdb: fewer copies inside existing output paths')
plot('shared',[('shared','ns',1e6,'Owned baseline vs shared result (ms)'),('shared','calls',1000,'Allocation calls (thousands)'),('shared','bytes',2**20,'Cumulative requested MiB'),('shared','buffer_current',2**20,'Held budget reservation (MiB)')],'Internal prototype: shared result has a DIFFERENT ownership contract')
print('\n'.join(lines))
