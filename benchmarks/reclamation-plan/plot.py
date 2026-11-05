"""Keep light-instrumentation acceptance results separate from diagnostic runs."""
import json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=pathlib.Path(sys.argv[1]);plt.rcParams['svg.hashsalt']='catdb-reclamation-plan'
cohort=sys.argv[3] if len(sys.argv)>3 else 'native'
rows=json.loads((root/cohort/'computed.json').read_text(encoding='utf-8'))
raw=json.loads((root/cohort/'results.json').read_text(encoding='utf-8'))
for r in rows:
 source=next(x for x in raw if (x['variant'],x['scenario'],x['rep'])==(r['variant'],r['scenario'],r['rep']))
 r['worker_hold_ms']=source['worker_max_lock_hold_ns']/1e6
fig,axes=plt.subplots(2,4,figsize=(15,8),layout='constrained')
for row,(suffix,label) in enumerate([('-light','Light: acceptance comparison'),('','Detailed diagnostics')]):
 for ax,metric,title in zip(axes[row],['readers_p99_ms','readers_max_ms','total_gc_ms','worker_hold_ms'],['Read p99','Worst read','Complete reclamation','Worst worker lock hold']):
  worst=metric in ('readers_max_ms','worker_hold_ms')
  for i,(variant,color) in enumerate([('before'+suffix,'#777777'),('after'+suffix,'#0072B2')]):
   values=[r[metric] for r in rows if r['scenario']=='gc' and r['variant']==variant];v=(max if worst else statistics.median)(values)
   ax.bar(i,v,color=color,width=.65)
   ax.annotate(f'{v:.3f}',(i,v if worst else max(values)),xytext=(0,4),textcoords='offset points',ha='center',fontsize=10)
   if not worst:ax.plot([i,i],[min(values),max(values)],color='black',lw=1)
  ax.set_title(label+'\n'+title,fontsize=11);ax.set_ylabel('ms; lower is better');ax.set_xticks([0,1],['Previous','Prototype']);ax.set_ylim(0,ax.get_ylim()[1]*1.2)
fig.suptitle((sys.argv[2] if len(sys.argv)>2 else 'Two-phase reclamation preparation: prototype NOT adopted')+'\n100,000 records | two readers | 3 paired repeats; medians / worst across repeats',fontsize=15)
fig.savefig(root/(cohort+'.svg'));fig.savefig(root/(cohort+'.png'),dpi=140)
p=root/(cohort+'.svg');p.write_text('\n'.join(line.rstrip() for line in p.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
