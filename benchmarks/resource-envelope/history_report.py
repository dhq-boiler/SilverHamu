"""Validate and summarize the paired initial-history-capacity experiment."""
import argparse, collections, gzip, importlib.util, json, pathlib, statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser=argparse.ArgumentParser()
parser.add_argument('--input',type=pathlib.Path,required=True)
parser.add_argument('--out',type=pathlib.Path,required=True)
a=parser.parse_args()
spec=importlib.util.spec_from_file_location('resource_report',pathlib.Path(__file__).with_name('report.py'))
mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)
manifest=json.loads((a.input/'manifest.json').read_text(encoding='utf8'))
assert len(manifest['runs'])==12 and all(r['status']=='completed' for r in manifest['runs'])
a.out.mkdir(parents=True,exist_ok=True)
cases=[];counts=collections.Counter()
for run in manifest['runs']:
    root=a.input/run['directory'];cs,totals=mod.summary(root)
    assert len(cs)==(4 if run['scale']=='million' else 1)
    for c in cs:
        assert c['status'] in ['passed','failed']
        if c['status']=='failed':
            assert c['failure_kind']=='oom' and c.get('rescue_verified'), (run['directory'],c)
        else:
            assert len(c['verification'])==4 and all(v['passed'] for v in c['verification'])
        c.update(cohort=run['cohort'],scale=run['scale'],round=run['rep'],raw_directory=run['directory'])
    cases+=cs;counts.update(totals)
    for f in root.rglob('*'):
        if f.is_file():
            dest=a.out/'raw'/root.name/f.relative_to(root);dest.parent.mkdir(parents=True,exist_ok=True)
            if f.name=='environment.json': dest.write_bytes(f.read_bytes())
            else:
                with gzip.open(str(dest)+'.gz','wb') as target:target.write(f.read_bytes())
(a.out/'manifest.json').write_text(json.dumps(manifest,indent=2),encoding='utf8')
(a.out/'summary.json').write_text(json.dumps({'counts':dict(counts),'cases':cases},indent=2),encoding='utf8')
groups=collections.defaultdict(list)
for c in cases: groups[c['scale'],c['cpus'],c['memory_mib'],c['cohort']].append(c)
med=statistics.median
def metric(cs,kind):
    ok=[c for c in cs if c['status']=='passed']
    if kind=='peak': return [c['resource_summary']['boot']['cgroup_peak_mib'] for c in cs]
    if kind=='rss': return [c['resource_summary']['boot']['sampled_rss_max_mib'] for c in ok]
    if kind in ['boot','reopen']: return [c[kind]['timing']['launch_to_ready_ms'] for c in ok]
    work='read' if kind=='read' else 'mixed'
    ms=[next(m for m in c['measurements'] if m['work']==work) for c in ok]
    if kind=='write': return [m['by_kind']['write']['p99_ms'] for m in ms]
    if kind=='throughput': return [m['ops_s'] for m in ms]
    if kind=='reject': return [100*m['queue_rejected']/m['ops'] for m in ms]
    return [m['p99_ms'] for m in ms]
table=['| Rows | CPU | MiB limit | Version | Passed | Peak MiB | RSS MiB | Read p99 ms | Update p99 ms | Mixed req/s | Reject % | Launch ms | Reopen ms |',
       '|---:|---:|---:|:---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
for (scale,cpu,mem,cohort),cs in sorted(groups.items()):
    values=[]
    for key in ['peak','rss','read','write','throughput','reject','boot','reopen']:
        xs=metric(cs,key);values.append(f'{med(xs):.3f}' if xs else '—')
    table.append(f'| {cs[0]["rows"]} | {cpu} | {mem} | {cohort} | {sum(c["status"]=="passed" for c in cs)}/3 | '+ ' | '.join(values)+' |')
(a.out/'table.md').write_text('\n'.join(table)+'\n',encoding='utf8')
fig,axes=plt.subplots(1,3,figsize=(15,4.8))
cells=[(1,512),(1,1024),(2,512),(2,1024)]
for ax,key,title,ylabel in zip(axes,['peak','read','write'],['Container peak (OOM included)','Read-only p99','Update p99 in mixed workload'],['MiB','ms; scheduled arrival to response','ms; scheduled arrival to response']):
    for j,(cohort,color) in enumerate([('before','#8c8c8c'),('after','#2979b5')]):
        pos=[i+(j-.5)*.36 for i in range(4)];ys=[]
        for cpu,mem in cells:
            xs=metric(groups['million',cpu,mem,cohort],key);ys.append(med(xs) if xs else float('nan'))
        ax.bar(pos,ys,width=.34,label=cohort,color=color)
        for i,(cpu,mem) in enumerate(cells):
            cs=groups['million',cpu,mem,cohort];xs=metric(cs,key)
            if xs:
                ax.scatter([pos[i]+(k-1)*.045 for k in range(len(xs))],xs,c='black',s=10,zorder=3)
                if key=='peak':ax.text(pos[i],ys[i],f'{sum(c["status"]=="passed" for c in cs)}/3',ha='center',va='bottom',fontsize=8)
            else:ax.text(pos[i],0,'OOM',ha='center',va='bottom',rotation=90,fontsize=8)
    ax.set_xticks(range(4),[f'{c} CPU\n{m} MiB' for c,m in cells]);ax.set_title(title);ax.set_ylabel(ylabel);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True);ax.margins(y=.18)
axes[0].legend(loc='upper left', fontsize=8)
fig.suptitle('1,000,000 rows / 256 B / initial history capacity: default vs 1\n1,000 offered requests/s; medians of 3 trials; dots: trials; 640-MiB retained budget',fontsize=11)
fig.tight_layout();fig.savefig(a.out/'comparison.svg');fig.savefig(a.out/'comparison.png',dpi=140);plt.close(fig)
for svg in a.out.glob('*.svg'):svg.write_text('\n'.join(l.rstrip() for l in svg.read_text(encoding='utf8').splitlines())+'\n',encoding='utf8')
print(json.dumps(dict(counts),indent=2));print('\n'.join(table))
