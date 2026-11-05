import json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
p=pathlib.Path(sys.argv[1]);a=json.loads((p/'auxiliary/computed.json').read_text(encoding='utf-8'))
variants=['cooperative','background','cooperative-light','background-light']
names=['Cooperative / timed','Background / timed','Cooperative / light','Background / light']
fig,axes=plt.subplots(1,3,figsize=(15,4.3),layout='constrained')
for ax,metric,title,worst in zip(axes,['readers_p99_ms','readers_max_ms','total_gc_ms'],['Read p99','Worst read across all repeats','Release + complete reclamation'],[False,True,False]):
 groups=[[r[metric] for r in a if r['scenario']=='gc' and r['variant']==v] for v in variants]
 values=[max(g) if worst else statistics.median(g) for g in groups]
 errs=None if worst else [[v-min(g) for v,g in zip(values,groups)],[max(g)-v for v,g in zip(values,groups)]]
 bars=ax.barh(range(4),values,xerr=errs,color=['#767676','#0072B2']*2,capsize=3)
 for i,b in enumerate(bars):
  if i>=2:b.set_hatch('//');b.set_edgecolor('white')
 ax.set_yticks(range(4),names);ax.invert_yaxis();ax.set_title(title,fontweight='bold');ax.set_xlabel('Milliseconds - lower is better');ax.set_xlim(0,max(max(g) for g in groups)*1.28)
 for i,v in enumerate(values):ax.text(max(groups[i])+max(values)*.02,i,f'{v:.3f}',va='center')
 ax.spines[['top','right']].set_visible(False);ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True)
fig.suptitle('catdb: request latency versus complete reclamation time\n100,000 overwritten 256-B values | two readers | selected opt-in policy',fontsize=15,fontweight='bold')
fig.supxlabel('3 paired repeats. p99/total: median with min-max; worst read: maximum of every repeat.\nNative diagnostic workload, not the service API. Light omits state-lock timing; allocator accounting remains.',fontsize=10)
plt.rcParams['svg.hashsalt']='catdb-reclamation-native'
fig.savefig(p/'native.svg');fig.savefig(p/'native.png',dpi=140)
svg=p/'native.svg';svg.write_text('\n'.join(line.rstrip() for line in svg.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
