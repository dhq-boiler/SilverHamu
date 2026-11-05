"""Plot every snapshot-registration candidate, including lighter-instrumentation runs."""
import json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=pathlib.Path(sys.argv[1]);plt.rcParams['svg.hashsalt']='catdb-reclamation-pins'
fig,axes=plt.subplots(2,3,figsize=(14,9),layout='constrained')
for row,(folder,label) in enumerate([('native','Separate registry lock'),('atomic','Atomic shared-generation release')]):
 data=json.loads((root/folder/'computed.json').read_text(encoding='utf-8'))
 for ax,metric,title in zip(axes[row],['readers_p99_ms','readers_max_ms','total_gc_ms'],['Read p99','Worst read across all repeats','Complete reclamation']):
  for i,(variant,color) in enumerate([('before','#777777'),('after','#0072B2'),('before-light','#999999'),('after-light','#56B4E9')]):
   values=[r[metric] for r in data if r['scenario']=='gc' and r['variant']==variant];value=(max if metric=='readers_max_ms' else statistics.median)(values)
   ax.bar(i,value,color=color);ax.annotate(f'{value:.3f}',(i,max(values) if metric!='readers_max_ms' else value),xytext=(0,4),textcoords='offset points',ha='center',va='bottom',fontsize=9)
   if metric!='readers_max_ms':ax.plot([i,i],[min(values),max(values)],color='black',lw=1)
  ax.set_title(label+'\n'+title);ax.set_ylabel('ms; lower is better');ax.set_xticks(range(4),['Previous','Prototype','Previous\nlight','Prototype\nlight'],fontsize=9);ax.set_ylim(0,ax.get_ylim()[1]*1.17)
fig.suptitle('Snapshot-registration prototypes (not adopted): data locks per read 3 to 2\n100,000 overwritten records | two readers | medians of 3 runs; whiskers = min/max',fontsize=15)
fig.savefig(root/'native.svg');fig.savefig(root/'native.png',dpi=140)
p=root/'native.svg';p.write_text('\n'.join(line.rstrip() for line in p.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
