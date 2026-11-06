from pathlib import Path
import json,statistics,matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import argparse
parser=argparse.ArgumentParser();parser.add_argument('--report',type=Path,required=True);args=parser.parse_args()
root=args.report;data=json.loads((root/'summary.json').read_text())
plt.rcParams.update({'font.size':10,'svg.fonttype':'none'})
fig,axes=plt.subplots(2,3,figsize=(12,7))
items=[('ops_per_s','Throughput incl. transitions','updates/s; higher is better'),('request_p99_ms','Request p99 incl. transitions','ms; lower is better'),('request_max_ms','Maximum request per trial','ms; lower is better'),('switch_p99_ms','Transition p99','ms; lower is better'),('append_p99_ms','Append-only p99','ms; lower is better'),('reopen_p99_ms','Warm reopen p99','ms; lower is better')]
for ax,(key,title,label) in zip(axes.flat,items):
 for x,(mode,color) in enumerate([('clear','#2463a5'),('reuse','#b85916')]):
  rs=[r for r in data if r['mode']==mode]
  for r in rs:ax.scatter(x+(r['trial']-2)*.08,r[key],color=color,marker=['o','s','^'][r['trial']-1],s=42,zorder=3)
  med=statistics.median(r[key] for r in rs);ax.plot([x-.2,x+.2],[med,med],color=color,lw=2)
 ax.set_xticks([0,1],['Clear','Reuse']);ax.set_xlim(-.5,1.5);ax.set_ylim(bottom=0);ax.set_title(title);ax.set_ylabel(label);ax.grid(axis='y',alpha=.25)
fig.suptitle('Bounded dual-copy prototype: clear vs epoch reuse',fontsize=16)
fig.text(.5,.025,'Each point is one trial (circle 1, square 2, triangle 3); horizontal line = median.\nSingle writer; 2 CPU / 4 GiB; 2,000 updates + 500 transitions per trial. Not a production DBMS benchmark.',ha='center',fontsize=10)
fig.tight_layout(rect=(0,.085,1,.95));fig.savefig(root/'comparison.svg');fig.savefig(root/'comparison.png',dpi=130)
print('figure written')
