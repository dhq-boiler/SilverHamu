from pathlib import Path
import json,gzip,statistics,shutil
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import sys
out=Path(sys.argv[1])
a=json.loads((out/'summary.json').read_text())
fig,axs=plt.subplots(1,2,figsize=(10,3.8),layout='constrained')
labels=['1: off','2: on','3: on','4: off'];colors=['#546e7a','#bb641c','#bb641c','#546e7a']
for ax,metric,title in zip(axs,['p99','max'],['Write p99','Maximum write response']):
 vals=[x['write_ms'][metric] for x in a];bars=ax.bar(labels,vals,color=colors)
 ax.bar_label(bars,fmt='%.2f',padding=3);ax.set_ylabel('ms, from planned arrival');ax.set_title(title);ax.set_ylim(0,max(vals)*1.18)
 ax.spines[['top','right']].set_visible(False)
fig.suptitle('Retirement diagnostics pilot — 1M rows / 1 CPU\nTwo short trials per mode; no optimization claim',fontsize=12)
fig.savefig(out/'pilot.svg');fig.savefig(out/'pilot.png',dpi=150);plt.close(fig)
on=[x for x in a if x['mode']=='on']
fig,ax=plt.subplots(figsize=(9,3.6),layout='constrained')
names=['Old-value release','Writer-lock wait','Collection','Sync envelope']
keys=['free','writer_wait','collection','sync_envelope']
for i,x in enumerate(on):
 ax.plot(names,[x['phases_us'][k]['p99'] for k in keys],marker=['o','s'][i],linestyle='none',markersize=7,label=x['case'])
ax.set_yscale('log');ax.set_ylabel('microseconds, p99 (log scale)');ax.set_title('Same-request server phases — diagnostics enabled')
ax.legend();ax.grid(axis='y',alpha=.2);fig.savefig(out/'phases.svg');fig.savefig(out/'phases.png',dpi=150);plt.close(fig)
