import json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=pathlib.Path(sys.argv[1])
plt.rcParams['svg.hashsalt']='catdb-reclamation-cost'
rows=json.loads((root/'diagnosis/computed.json').read_text(encoding='utf-8'))
rows=[r for r in rows if r['worker_elapsed_ms']>0]
fig,ax=plt.subplots(figsize=(10,4.7),layout='constrained')
bottom=[0]*3
for key,label,color in [('work_ms','Collection / lock scope','#0072B2'),('free_ms','Free retired values','#009E73'),('yield_ms','Yield','#CC79A7'),('pause_elapsed_ms','Timed pauses (actual)','#E69F00')]:
 values=[r[key] for r in rows];ax.bar(['Run 1','Run 2','Run 3'],values,bottom=bottom,label=label,color=color);bottom=[a+b for a,b in zip(bottom,values)]
ax.bar(['Run 1','Run 2','Run 3'],[r['worker_elapsed_ms']-b for r,b in zip(rows,bottom)],bottom=bottom,label='Other worker time',color='#999999')
ax.set_ylabel('Worker wall time (ms); lower is better');ax.set_title('Why reclamation took hundreds of milliseconds\nOriginal background policy; detailed profiling enabled');ax.legend(loc='upper left');ax.set_ylim(0,510)
fig.savefig(root/'diagnosis.svg');fig.savefig(root/'diagnosis.png',dpi=140);plt.close(fig)
fig,axes=plt.subplots(1,3,figsize=(13,4.8),layout='constrained')
rows=json.loads((root/'fast-full/computed.json').read_text(encoding='utf-8'))
for ax,metric,title in zip(axes,['readers_p99_ms','readers_max_ms','total_gc_ms'],['Read p99','Worst read across all three runs','Complete reclamation']):
 for i,(variant,label,color) in enumerate([('before','Previous','#777777'),('after','Prototype','#0072B2'),('before-light','Previous\nlight','#999999'),('after-light','Prototype\nlight','#56B4E9')]):
  values=[r[metric] for r in rows if r['scenario']=='gc' and r['variant']==variant];v=(max if metric=='readers_max_ms' else statistics.median)(values)
  ax.bar(i,v,color=color);ax.text(i,v,f'{v:.3f}',ha='center',va='bottom',fontsize=9)
  if metric!='readers_max_ms':ax.plot([i,i],[min(values),max(values)],color='black',lw=1)
 ax.set_xticks(range(4),['Previous','Prototype','Previous\nlight','Prototype\nlight'],fontsize=9);ax.set_ylabel('ms; lower is better');ax.set_title(title);ax.set_ylim(0,ax.get_ylim()[1]*1.18)
fig.suptitle('Faster collection did not preserve the read tail — prototype NOT adopted\n100,000 overwritten records | two readers | three paired repeats',fontsize=14)
fig.savefig(root/'native.svg');fig.savefig(root/'native.png',dpi=140);plt.close(fig)

for name in ("diagnosis.svg", "native.svg"):
 p=root/name
 p.write_text("\n".join(line.rstrip() for line in p.read_text(encoding="utf-8").splitlines())+"\n",encoding="utf-8")
