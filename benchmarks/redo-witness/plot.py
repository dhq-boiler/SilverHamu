import sys
from pathlib import Path
from statistics import median
from verify import verify

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

root = Path(sys.argv[1])
audit = verify(root / 'comparison')
names = {'catdb-baseline':'catdb append','catdb-service':'catdb redo','postgres':'PostgreSQL','oracle':'Oracle Free'}
colors = ['#8d969c','#006992','#33704b','#955146']
fig, axes = plt.subplots(2, 2, figsize=(9.4, 5.6), sharey=True, layout='constrained')
plt.rcParams['svg.hashsalt'] = 'catdb-redo-witness-comparison'
for i, workload in enumerate(audit['workloads']):
    for j, (metric, text) in enumerate([('trials_ops','Durable updates/s'),('trials_p99_ms','p99 latency (ms)')]):
        ax = axes[i,j]
        for y, ((key, label), color) in enumerate(zip(names.items(), colors)):
            v = workload['engines'][key][metric]
            ax.hlines(-y,min(v),max(v),color=color,lw=2)
            ax.plot(median(v),-y,'o',color=color,markersize=7)
        ax.set_title(f"{workload['clients']} clients · {text}")
        ax.set_yticks([0,-1,-2,-3], list(names.values()))
        ax.set_xlim(left=0)
        ax.spines[['right','top']].set_visible(False)
        ax.grid(axis='x',alpha=.2)
        ax.margins(y=.22)
fig.supxlabel('Dot = median of three runs   |   Line = minimum to maximum', fontsize=10)
fig.savefig(root/'comparison.svg', metadata={'Date':None})
plt.close(fig)
