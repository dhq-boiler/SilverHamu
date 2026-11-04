"""Render contemporaneous durable-update comparisons without hiding slow cases."""
import csv, json, pathlib, statistics, sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
plt.rcParams["svg.hashsalt"]="catdb-log-sync-2026-11-05"
root=pathlib.Path(sys.argv[1])
rows=json.loads((root/"results.json").read_text(encoding="utf-8"))
env=json.loads((root/"environment.json").read_text(encoding="utf-8"))
assert len(rows)==24 and len(env['runs'])==12 and all(r['status']=='passed' for r in env['runs'])
names={'catdb-current':'catdb before','catdb-candidate':'catdb after','postgres':'PostgreSQL','oracle':'Oracle Free'}
colors={'catdb-current':'#767676','catdb-candidate':'#0072B2','postgres':'#009E73','oracle':'#CC79A7'}
fig,axes=plt.subplots(2,2,figsize=(13,8),layout='constrained')
fig.suptitle('Durable service updates: catdb, PostgreSQL and Oracle Free\n100,000 records | 256-byte values | same original Python client',fontsize=16,fontweight='bold')
summary=[]
for line,clients in enumerate((1,4)):
    cells=[]
    for engine in names:
        selected=[r for r in rows if r['engine']==engine and r['clients']==clients and r['workload']=='update']
        assert len(selected)==3 and all(not r['errors'] for r in selected)
        cell={'engine':engine,'clients':clients}
        for metric in ('ops_s','p50_ms','p95_ms','p99_ms','max_ms'):
            values=[r[metric] for r in selected]
            cell[metric]=statistics.median(values)
            cell[metric+'_min']=min(values);cell[metric+'_max']=max(values)
        cells.append(cell);summary.append(cell)
    for col,metric in enumerate(('ops_s','p99_ms')):
        ax=axes[line,col]
        values=[c[metric] for c in cells]
        error=[[c[metric]-c[metric+'_min'] for c in cells],[c[metric+'_max']-c[metric] for c in cells]]
        ax.barh(range(4),values,xerr=error,color=[colors[c['engine']] for c in cells],capsize=3,height=.65)
        ax.set_yticks(range(4),[names[c['engine']] for c in cells]);ax.invert_yaxis()
        ax.set_title(f'{clients} client'+('s' if clients==4 else ''),loc='left',fontweight='bold')
        ax.set_xlabel('Operations / second - higher is better' if metric=='ops_s' else 'p99 complete update response (ms) - lower is better')
        maximum=max(c[metric+'_max'] for c in cells);ax.set_xlim(0,maximum*1.25)
        ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True);ax.spines[['top','right']].set_visible(False)
        for y,c in enumerate(cells):
            label=f"{c[metric]:,.0f}" if metric=='ops_s' else f"{c[metric]:.3f}"
            ax.text(c[metric+'_max']+maximum*.015,y,label,va='center')
pg=next(r['metadata']['select version()'][0][0].split()[1] for r in env['runs'] if r['engine']=='postgres')
ora=next(r['metadata']['select banner_full from v$version'][0][0].split('Version ')[-1] for r in env['runs'] if r['engine']=='oracle')
fig.supxlabel('Median of 3 runs; whiskers = min/max. All four paths rerun in varied order; timing instrumentation disabled.\nExperimental API microbenchmarks, not a general DBMS ranking. Sync before acknowledgement is retained.\nPostgreSQL '+pg+' | Oracle Free '+ora+' | catdb 0.1.0 (before/after) | 2026-11-05 JST',fontsize=9)
fig.savefig(root/'service-updates.svg',metadata={'Date':env['utc']});fig.savefig(root/'service-updates.png',dpi=140);plt.close(fig)
p=root/'service-updates.svg';p.write_text('\n'.join(line.rstrip() for line in p.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
with (root/'summary.csv').open('w',newline='',encoding='utf-8') as f:
    writer=csv.DictWriter(f,fieldnames=list(summary[0]));writer.writeheader();writer.writerows(summary)
table=['| API | Clients | ops/s | p50 ms | p95 ms | p99 ms |','|---|---:|---:|---:|---:|---:|']
for c in summary:table.append(f"| {names[c['engine']]} | {c['clients']} | {c['ops_s']:.1f} | {c['p50_ms']:.3f} | {c['p95_ms']:.3f} | {c['p99_ms']:.3f} |")
(root/'table.md').write_text('\n'.join(table)+'\n',encoding='utf-8')
