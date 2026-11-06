import json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=pathlib.Path(sys.argv[1]);rs=json.loads((root/'summary.json').read_text());cs=json.loads((root/'classes.json').read_text())
names=['catdb-plain','catdb-trace','sqlite-plain','sqlite-trace','sqlite-pinned-plain','sqlite-pinned-trace']
labels=['catdb','catdb + trace','SQLite','SQLite + trace','SQLite, old reader','SQLite, old reader + trace']
plt.rcParams.update({'font.size':10,'svg.fonttype':'none'})
fig,axs=plt.subplots(1,3,figsize=(15,5),layout='constrained')
for ax,key,title in zip(axs,['updates_s','p99_ms','max_ms'],['Updates / second (higher is better)','Request p99, ms (lower is better)','Maximum request, ms (lower is better)']):
 vals=[[r[key] for r in rs if r['variant']==n] for n in names]
 ax.barh(range(6),[statistics.median(v) for v in vals],color=['#287d8e','#87bcc4','#bc722f','#e7b888','#7159a3','#b4a6d0'])
 for i,v in enumerate(vals):ax.scatter(v,[i]*len(v),color='#202633',s=19,zorder=3)
 ax.set_yticks(range(6),labels if ax==axs[0] else ['']*6);ax.invert_yaxis();ax.set_title(title);ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True)
fig.suptitle('100k rows / 256 B / one writer / 2 CPU / 4 GiB\nBars: median of 3 trials; dots: every trial. Traced runs are observer comparisons.')
for ext in ['svg','png']:fig.savefig(root/f'comparison.{ext}',dpi=150)
plt.close(fig)
cats=[('catdb-trace','growing','catdb: file grows'),('sqlite-trace','growing','SQLite: file grows'),('sqlite-trace','reuse','SQLite: existing area'),('sqlite-pinned-trace','growing','SQLite: old reader, grows')]
fig,ax=plt.subplots(figsize=(10,4),layout='constrained')
for i,(variant,kind,label) in enumerate(cats):
 vals=[c['classes'][kind]['mean_sync_ms'] for c in cs if c['file'].endswith('-'+variant+'.json.gz')]
 ax.barh(i,statistics.median(vals),color='#287d8e' if variant.startswith('catdb') else '#bc722f');ax.scatter(vals,[i]*len(vals),s=25,color='#202633',zorder=3)
ax.set_yticks(range(4),[x[2] for x in cats]);ax.invert_yaxis();ax.set_xlabel('Observed synchronization elapsed time per request, ms');ax.grid(axis='x',alpha=.2);ax.set_axisbelow(True)
ax.set_title('Synchronization stays enabled; calls become shorter in SQLite reused regions\nMedian of per-trial class means; DB-write/checkpoint requests excluded')
for ext in ['svg','png']:fig.savefig(root/f'sync-classes.{ext}',dpi=150)
plt.close(fig)
lines=['| Condition | Updates/s | p99 ms | Worst maximum ms |','|---|---:|---:|---:|']
for n in names:
 r=[r for r in rs if r['variant']==n];lines.append(f"| {n} | {statistics.median(x['updates_s'] for x in r):.3f} | {statistics.median(x['p99_ms'] for x in r):.3f} | {max(x['max_ms'] for x in r):.3f} |")
(root/'table.md').write_text('\n'.join(lines)+'\n')
for p in root.glob('*.svg'):p.write_text('\n'.join(line.rstrip() for line in p.read_text().splitlines())+'\n')
print('\n'.join(lines))
