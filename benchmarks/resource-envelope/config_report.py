import argparse,collections,gzip,hashlib,importlib.util,json,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser=argparse.ArgumentParser();parser.add_argument('--artifacts',type=pathlib.Path,required=True);parser.add_argument('--out',type=pathlib.Path,required=True);args=parser.parse_args()
repo=pathlib.Path(__file__).resolve().parents[2];base=args.artifacts
spec=importlib.util.spec_from_file_location('resource_report',repo/'benchmarks/resource-envelope/report.py');mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)
out=args.out;out.mkdir(parents=True,exist_ok=True)
cases=[];counts={}
for name in ['resource-config-default','resource-config-million']:
    root=base/name;rows,total=mod.summary(root)
    assert all(c['status'] in ['passed','failed'] for c in rows)
    for c in rows:c['cohort']=name
    cases+=rows;counts[name]=total
    for f in root.rglob('*'):
        if f.is_file():
            dest=out/'raw'/name/f.relative_to(root);dest.parent.mkdir(parents=True,exist_ok=True)
            if f.name=='environment.json':dest.write_bytes(f.read_bytes())
            else:
                with gzip.open(str(dest)+'.gz','wb') as g:g.write(f.read_bytes())
(out/'summary.json').write_text(json.dumps({'counts':counts,'cases':cases},indent=2),encoding='utf8')
groups=collections.defaultdict(list)
for c in cases:
    if c['cohort']=='resource-config-million':groups[c['cpus'],c['memory_mib']].append(c)
med=statistics.median
table=['| CPU | Container MiB | Passed / attempted | Read p99 ms | Mixed p99 ms | Peak cgroup MiB | Launch ms | Reopen launch ms |','|---:|---:|---:|---:|---:|---:|---:|---:|']
labels=[];lat=[];memory=[];points=[];availability=[]
for (cpu,mem),cs in sorted(groups.items()):
    ok=[c for c in cs if c['status']=='passed'];labels.append(f'{cpu} CPU\n{mem} MiB');availability.append(len(ok));memory.append(med(c['resource_summary']['boot']['cgroup_peak_mib'] for c in cs))
    if ok:
        reads=[next(m for m in c['measurements'] if m['work']=='read')['p99_ms'] for c in ok]
        mixes=[next(m for m in c['measurements'] if m['work']=='mixed')['p99_ms'] for c in ok]
        lat.append(med(mixes));points.append(mixes)
        nums=[med(reads),med(mixes),memory[-1],med(c['boot']['timing']['launch_to_ready_ms'] for c in ok),med(c['reopen']['timing']['launch_to_ready_ms'] for c in ok)]
        values=' | '.join(f'{x:.3f}' for x in nums)
    else:lat.append(float('nan'));points.append([]);values=f'— | — | {memory[-1]:.3f} | — | —'
    table.append(f'| {cpu} | {mem} | {len(ok)} / {len(cs)} | {values} |')
fig,axes=plt.subplots(1,2,figsize=(12,4.5))
axes[0].bar(range(len(labels)),lat,color='#3678aa');axes[1].bar(range(len(labels)),memory,color=['#3678aa' if n else '#b54938' for n in availability])
for i,ys in enumerate(points):
    if ys:axes[0].scatter([i+(j-1)*.12 for j in range(len(ys))],ys,c='#222',s=18,zorder=3)
    else:axes[0].text(i,0,'No startup',ha='center',va='bottom',rotation=90,color='#b54938')
for i,n in enumerate(availability):axes[1].text(i,memory[i],f'{n}/3 passed',ha='center',va='bottom',fontsize=9)
for ax,title,ylabel in zip(axes,['Mixed workload: scheduled-arrival p99','Server cgroup peak (including failed startup)'],['ms; only passing cases','MiB; includes observer and lifecycle']):
    ax.set_xticks(range(len(labels)),labels);ax.set_title(title);ax.set_ylabel(ylabel);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True);ax.margins(y=.18)
fig.suptitle('1,000,000 rows / 256 B / explicit 640-MiB retained budget\n1,000 offered requests/s; bars: trial medians; dots: individual p99 trials',fontsize=11)
fig.tight_layout();fig.savefig(out/'comparison.svg');fig.savefig(out/'comparison.png',dpi=140);plt.close(fig)
for f in out.glob('*.svg'):f.write_text('\n'.join(x.rstrip() for x in f.read_text(encoding='utf8').splitlines())+'\n',encoding='utf8')
(out/'table.md').write_text('\n'.join(table)+'\n',encoding='utf8')
files=['src/service.rs','src/bin/silverhamu-server.rs','tests/service_protocol.py','benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/supervisor.py','benchmarks/resource-envelope/client.py','benchmarks/resource-envelope/report.py','benchmarks/resource-envelope/config_report.py']
(out/'source-sha256.json').write_text(json.dumps({'normalization':'CRLF to LF','files':{n:hashlib.sha256((repo/n).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for n in files}},indent=2),encoding='utf8')
print(json.dumps(counts,indent=2));print('\n'.join(table))
