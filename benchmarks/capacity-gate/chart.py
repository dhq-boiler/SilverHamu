from pathlib import Path
import argparse,json,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
p=argparse.ArgumentParser();p.add_argument('--report',type=Path,required=True);a=p.parse_args();root=a.report;data=json.loads((root/'summary.json').read_text())
plt.rcParams.update({'font.size':10,'svg.fonttype':'none'})
fig,axes=plt.subplots(2,3,figsize=(12,7))
items=[('ops_per_s','Throughput including transitions','updates/s; higher is better'),('request_p99_ms','Request p99','ms; lower is better'),('request_max_ms','Maximum request per trial','ms; lower is better'),('reopen_p50_ms','Warm reopen at full occupancy','median ms; lower is better'),('file_bytes','Reserved file size','MiB; lower is smaller'),('sync_calls','Sync calls per 2,048 updates','calls; lower is fewer')]
for ax,(key,title,label) in zip(axes.flat,items):
 for x,(cap,color) in enumerate([(4,'#2463a5'),(16,'#16806a'),(64,'#b85916')]):
  rs=[r for r in data if r['capacity']==cap];div=1048576 if key=='file_bytes' else 1
  for r in rs:ax.scatter(x+(r['trial']-2)*.08,r[key]/div,color=color,marker=['o','s','^'][r['trial']-1],s=42,zorder=3)
  m=statistics.median(r[key]/div for r in rs);ax.plot([x-.2,x+.2],[m,m],color=color,lw=2)
 ax.set_xticks([0,1,2],['4','16','64']);ax.set_xlabel('Updates between transitions');ax.set_ylim(bottom=0);ax.set_xlim(-.5,2.5);ax.set_title(title);ax.set_ylabel(label);ax.grid(axis='y',alpha=.2)
fig.suptitle('Bounded prototype: transition frequency versus recovery cost',fontsize=16)
fig.text(.5,.02,'Every trial shown (circle 1, square 2, triangle 3); horizontal line = median.\nSingle writer; 2 CPU / 4 GiB; 2,048 updates; real sync after each update. Small fixed-slot fixture only.',ha='center',fontsize=10)
fig.tight_layout(rect=(0,.085,1,.95));fig.savefig(root/'comparison.svg');fig.savefig(root/'comparison.png',dpi=130)
