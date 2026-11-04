"""Render medians and all-run ranges for the same durable update workload."""
import csv,json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
plt.rcParams['svg.hashsalt']='catdb-fixed-log-2026-11-05'
root=pathlib.Path(sys.argv[1]);rows=json.loads((root/'results.json').read_text(encoding='utf-8'));env=json.loads((root/'environment.json').read_text(encoding='utf-8'))
assert len(rows)==12 and all(r['status']=='passed' for r in env['runs'])
names={'current':'Current append log','fixed':'Fixed-length prototype'};colors={'current':'#767676','fixed':'#0072B2'}
fig,axes=plt.subplots(2,2,figsize=(12,7),layout='constrained');summary=[]
fig.suptitle('catdb: append log vs fixed-length prototype\n100,000 records | 256-byte values | durable single-row updates',fontsize=16,fontweight='bold')
for line,clients in enumerate((1,4)):
    cells=[]
    for variant in names:
        selected=[r for r in rows if r['variant']==variant and r['clients']==clients];assert len(selected)==3
        cell={'variant':variant,'clients':clients}
        for metric in ('ops_s','p50_ms','p95_ms','p99_ms','max_ms','application_written_bytes'):
            v=[r[metric] for r in selected];cell[metric]=statistics.median(v);cell[metric+'_min']=min(v);cell[metric+'_max']=max(v)
        cells.append(cell);summary.append(cell)
    for col,metric in enumerate(('ops_s','p99_ms')):
        ax=axes[line,col];v=[c[metric] for c in cells];err=[[c[metric]-c[metric+'_min'] for c in cells],[c[metric+'_max']-c[metric] for c in cells]]
        ax.barh(range(2),v,xerr=err,color=[colors[c['variant']] for c in cells],capsize=4,height=.55);ax.set_yticks(range(2),[names[c['variant']] for c in cells]);ax.invert_yaxis();ax.set_title(f'{clients} client'+('s' if clients==4 else ''),loc='left',fontweight='bold');ax.set_xlabel('Operations / second - higher is better' if metric=='ops_s' else 'p99 complete response (ms) - lower is better')
        mx=max(c[metric+'_max'] for c in cells);ax.set_xlim(0,mx*1.25);ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True);ax.spines[['top','right']].set_visible(False)
        for y,c in enumerate(cells):ax.text(c[metric+'_max']+mx*.02,y,f"{c[metric]:,.0f}" if metric=='ops_s' else f"{c[metric]:.3f}",va='center')
fig.supxlabel('Median of 3 runs; whiskers = min/max. Instrumentation off; same original Python client.\nFixed prototype includes BOTH payload and logical-end syncs. 64 MiB preinitialized capacity.\nExperimental warm-cache Docker/Linux API benchmark; not a general DBMS ranking. 2026-11-05 JST',fontsize=9)
fig.savefig(root/'fixed-log.svg',metadata={'Date':env['utc']});fig.savefig(root/'fixed-log.png',dpi=140);plt.close(fig)
p=root/'fixed-log.svg';p.write_text('\n'.join(line.rstrip() for line in p.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
with (root/'summary.csv').open('w',newline='',encoding='utf-8') as f:w=csv.DictWriter(f,fieldnames=list(summary[0]));w.writeheader();w.writerows(summary)
t=['| Mode | Clients | Updates/s | p50 ms | p95 ms | p99 ms | Application log bytes / 8,000 updates |','|---|---:|---:|---:|---:|---:|---:|']
for c in summary:t.append(f"| {names[c['variant']]} | {c['clients']} | {c['ops_s']:.1f} | {c['p50_ms']:.3f} | {c['p95_ms']:.3f} | {c['p99_ms']:.3f} | {c['application_written_bytes']:,.0f} |")
(root/'table.md').write_text('\n'.join(t)+'\n',encoding='utf-8')
