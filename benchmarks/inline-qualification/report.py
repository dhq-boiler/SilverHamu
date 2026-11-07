"""Matched qualification, preserving every trial and the pre-agreed gate."""
import importlib.util,json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('aged',ROOT/'benchmarks/aged-service/analyze.py')
aged=importlib.util.module_from_spec(spec);spec.loader.exec_module(aged)
root=pathlib.Path(sys.argv[1]);groups={};table=[];decisions={}
def metrics(t):
    m=t['workloads']['mixed'];v=m['by_kind']
    return {'read':v['read']['response_ms']['p99'],'write':v['write']['response_ms']['p99'],
            'max':max(x['response_ms']['max'] for x in v.values()),'rss':m['resources']['rss_peak_mib'],
            'cgroup':m['resources']['cgroup_peak_mib'],'reopen':t['reopen_ready_ms'],
            'refused':sum(w['queue_rejected'] for w in t['workloads'].values()),'ops_s':m['ops_s']}
for shape in ('short','mixed'):
    s=aged.summarize(root/shape/'raw',pin_reader=True)
    assert len(s['trials'])==6 and s['requests']==96000 and s['full_checks']==30
    (root/shape/'summary.json').write_text(json.dumps(s,indent=2))
    g={v:[metrics(t) for t in s['trials'] if t['variant']==v] for v in ('baseline','inline')};groups[shape]=g
    med={v:{k:statistics.median(x[k] for x in rows) for k in rows[0]} for v,rows in g.items()}
    delta={k:(med['inline'][k]/med['baseline'][k]-1)*100 for k in ('read','write','rss','reopen')}
    passed=delta['read']<=5 and delta['write']<=5 and all(x['refused']==0 for rows in g.values() for x in rows)
    if shape=='short':passed=passed and delta['rss']<=-5
    decisions[shape]={'medians':med,'delta_percent':delta,'numeric_gate_passed':passed}
    for t in s['trials']:
        m=metrics(t);table.append('| '+shape+' / '+t['label']+' | '+' | '.join(f'{m[k]:.3f}' for k in ('read','write','max','rss','cgroup','reopen','ops_s','refused'))+' |')
(root/'decision.json').write_text(json.dumps(decisions,indent=2))
(root/'table.md').write_text('| Shape / trial | Read p99 ms | Update p99 ms | Mixed max ms | RSS MiB | Cgroup MiB | Reopen ms | Updates + reads/s | Refusals |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n'+'\n'.join(table)+'\n')
fig,axes=plt.subplots(2,4,figsize=(13,7),layout='constrained')
for row,shape in enumerate(('short','mixed')):
    for ax,k,title,unit in zip(axes[row],('read','write','rss','reopen'),('Read p99','Update p99','Server RSS','Warm reopen readiness'),('ms','ms','MiB','ms')):
        arr=[[x[k] for x in groups[shape][v]] for v in ('baseline','inline')]
        bars=ax.bar(['Vec','Inline'],[statistics.median(v) for v in arr],color=['#546e7a','#197d83'])
        for i,values in enumerate(arr):ax.scatter([i-.07,i,i+.07],values,c='black',s=16,zorder=3)
        ax.bar_label(bars,fmt='%.2f',padding=3);ax.set_ylim(0,max(map(max,arr))*1.25);ax.set_ylabel(unit);ax.set_title(shape+' / '+title)
fig.suptitle('Inline-key default qualification: aged 1M rows, 1 CPU, 512 MiB\nPinned old reader, 1000 arrivals/s. Bars: median; dots: every trial.')
fig.savefig(root/'comparison.svg');fig.savefig(root/'comparison.png',dpi=140)
p=root/'comparison.svg';p.write_text('\n'.join(x.rstrip() for x in p.read_text().splitlines())+'\n')
print(json.dumps(decisions,indent=2))
