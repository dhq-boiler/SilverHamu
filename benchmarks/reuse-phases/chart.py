from pathlib import Path
import argparse,json,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
p=argparse.ArgumentParser();p.add_argument('--report',type=Path,required=True);a=p.parse_args();root=a.report;data=json.loads((root/'summary.json').read_text());main=[r for r in data if r['stage']=='main'];pilot=[r for r in data if r['stage']=='pilot']
plt.rcParams.update({'font.size':10,'svg.fonttype':'none'})
fig,axes=plt.subplots(1,3,figsize=(14,4.9),gridspec_kw={'width_ratios':[1,1.7,1.2]})
for x,(mode,color) in enumerate([('clear','#2463a5'),('reuse','#b85916')]):
 rs=[r for r in main if r['mode']==mode];vs=[r['total_p99_ms'] for r in rs]
 axes[0].scatter([x+(r['trial']-4)*.09 for r in rs],vs,color=color,s=45);m=statistics.median(vs);axes[0].plot([x-.2,x+.2],[m,m],color=color)
axes[0].set_xticks([0,1],['Clear','Reuse']);axes[0].set_title('Request p99, each main trial');axes[0].set_ylabel('ms');axes[0].set_ylim(bottom=0);axes[0].grid(axis='y',alpha=.2)
ordered=sorted(main,key=lambda r:(r['mode'],r['trial']));bottom=[0]*len(ordered)
for label,key,color in [('Prepare','worst_prepare_ms','#6b6b6b'),('Write envelope','worst_write_ms','#60a5bf'),('Sync envelope','worst_sync_ms','#bd5916'),('Publish','worst_publish_ms','#8060ac'),('Residual','worst_residual_ms','#a2a2a2')]:
 vs=[r[key] for r in ordered];axes[1].bar(range(len(vs)),vs,bottom=bottom,label=label,color=color);bottom=[x+y for x,y in zip(bottom,vs)]
axes[1].set_xticks(range(6),[r['mode']+' '+str(r['trial']-2) for r in ordered],rotation=30);axes[1].set_title('Same-request breakdown of each trial maximum');axes[1].set_ylabel('ms');axes[1].legend(fontsize=8,loc='upper right');axes[1].grid(axis='y',alpha=.2)
for x,(mode,en) in enumerate([('clear',False),('clear',True),('reuse',False),('reuse',True)]):
 rs=[r for r in pilot if r['mode']==mode and r['trace']==en];axes[2].scatter([x+(r['trial']-1.5)*.14 for r in rs],[r['total_p99_ms'] for r in rs],s=42,color='#2463a5' if mode=='clear' else '#bd5916')
axes[2].set_xticks(range(4),['Clear\noff','Clear\non','Reuse\noff','Reuse\non']);axes[2].set_title('Observer pilot: request p99');axes[2].set_ylabel('ms');axes[2].set_ylim(bottom=0);axes[2].grid(axis='y',alpha=.2)
fig.suptitle('Bounded reuse prototype: request phase timing',fontsize=16)
fig.text(.5,.025,'Main: 2,000 updates / trial, 3 trials / mode. Pilot: 400 updates, 2 trials / mode / observer setting.\nSync envelopes include scheduling and instrumentation; they are not measured physical disk I/O.',ha='center',fontsize=10)
fig.tight_layout(rect=(0,.11,1,.94));fig.savefig(root/'phases.svg');fig.savefig(root/'phases.png',dpi=130)
