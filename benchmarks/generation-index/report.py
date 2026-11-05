import csv
import gzip
import json
import math
from pathlib import Path
from statistics import median
import sys

root = Path(sys.argv[1])
output = Path(sys.argv[2])
output.mkdir(parents=True, exist_ok=True)
summary = []
for path in sorted(root.glob('*.csv')):
    size, shape, repeat = path.stem.split('-')
    groups = {}
    raw = path.read_bytes()
    (output / (path.name + '.gz')).write_bytes(gzip.compress(raw, mtime=0))
    for row in csv.DictReader(raw.decode().splitlines()):
        groups.setdefault((row['method'], row['case']), []).append(row)
    for (method, case), rows in groups.items():
        values = sorted(int(r['ns']) for r in rows)
        summary.append(dict(rows=int(size), shape=shape, repeat=int(repeat), method=method, case=case,
                            samples=len(values), median_ns=median(values), p99_ns=values[math.ceil(.99*len(values))-1],
                            max_ns=values[-1], bytes=int(rows[0]['bytes'])))
assert len(list(root.glob('*.csv'))) == 12
(output / 'summary.json').write_text(json.dumps(summary, indent=2), encoding='utf-8')

def value(size, shape, method, case, field='p99_ns'):
    vals=[r[field] for r in summary if (r['rows'],r['shape'],r['method'],r['case'])==(size,shape,method,case)]
    assert len(vals)==3, (size,shape,method,case)
    return median(vals)

lines=['| Rows | Updates | Method | Build ms | Update p99 µs | Rank100 p99 µs | Current MiB | With old root MiB | Allocated/update KiB |',
       '|---:|---|---|---:|---:|---:|---:|---:|---:|']
for n in [100000,1000000]:
    for shape in ['local','spread']:
        for method in ['count_only','count_max']:
            vals=[value(n,shape,method,'build')/1e6,value(n,shape,method,'update')/1e3,
                  value(n,shape,method,'rank100')/1e3,value(n,shape,method,'retained_current','bytes')/2**20,
                  value(n,shape,method,'retained_pinned','bytes')/2**20,value(n,shape,method,'update','bytes')/2400/1024]
            lines.append(f'| {n:,} | {shape} | {method} | '+' | '.join(f'{v:.3f}' for v in vals)+' |')
lines += ['', '| Rows | Updates | Range case | Count-only p99 µs | Global guard p99 µs | Count+max p99 µs | BTreeMap p99 µs |',
          '|---:|---|---|---:|---:|---:|---:|']
for n in [100000,1000000]:
    for shape in ['local','spread']:
        for case in ['stable_wide','stable_narrow','stale_wide','stale_narrow','stale_upper_half']:
            vals=[value(n,shape,m,case)/1e3 for m in ['count_only','count_global_guard','count_max','btree_map']]
            lines.append(f'| {n:,} | {shape} | {case} | '+' | '.join(f'{v:.3f}' for v in vals)+' |')
lines+=['','Sorted-vector rank100 p99 (µs; reference only): '+', '.join(f'{n:,}/{s}: {value(n,s,"sorted_vec","rank100")/1000:.3f}' for n in [100000,1000000] for s in ['local','spread'])]
(output/'tables.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
fig,axs=plt.subplots(2,2,figsize=(12,8),constrained_layout=True)
colors=['#64748b','#007f87']
for ax,case,label,field,scale in [(axs[0,0],'update','Update p99 (µs)','p99_ns',1000),
                                (axs[0,1],'rank100','Rank100 p99 (µs)','p99_ns',1000),
                                (axs[1,0],'retained_pinned','Retained with old root (MiB)*','bytes',2**20)]:
    for j,m in enumerate(['count_only','count_max']):
        ax.bar([i+(j-.5)*.32 for i in range(2)],[value(1000000,s,m,case,field)/scale for s in ['local','spread']],width=.30,color=colors[j],label=['Count only','Count + max generation'][j])
    ax.set_xticks([0,1],['Local updates','Spread updates']);ax.set_ylabel(label);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
ax=axs[1,1]
for j,m in enumerate(['count_global_guard','count_max','btree_map']):
    ax.bar([i+(j-1)*.24 for i in range(2)],[max(.1,value(1000000,s,m,'stale_upper_half')/1000) for s in ['local','spread']],width=.22,color=['#64748b','#007f87','#b75b18'][j],label=['Count + global guard','Count + max generation','BTreeMap scan'][j])
ax.set_xticks([0,1],['Local: unchanged range','Spread: conflicts']);ax.set_yscale('log');ax.set_ylabel('Upper-half range p99 (µs, log)');ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True);ax.legend(fontsize=8)
axs[0,0].legend(fontsize=9)
fig.suptitle('Experimental index: 1,000,000 rows · median of 3 trial p99s\nSingle thread, u64 keys/values; not a service or durability benchmark',fontsize=13)
fig.supxlabel('*Reachable node + Arc payload estimate, not process RSS or allocator peak. Raw trials accompany this chart.',fontsize=9)
fig.savefig(output/'comparison.svg');fig.savefig(output/'comparison.png',dpi=160)
print('\n'.join(lines[:12]))
