"""Publish all instrumented/control trials without treating maxima as guarantees."""
import gzip,json,shutil,statistics,sys
from pathlib import Path
from analyze import analyze_case

def main():
    source,out=map(Path,sys.argv[1:3]);out.mkdir(parents=True,exist_ok=True)
    rows=json.loads((source/'results.json').read_text());env=json.loads((source/'environment.json').read_text())
    assert all(x['status']=='passed' for x in env['runs'])
    assert all(not r['errors'] and r['attempted']==r['completed']+r['rejected'] for r in rows)
    analyses=[]
    for r in rows:
        events=json.load(gzip.open(source/r['events_file']));records=json.load(gzip.open(source/r['diagnostics_file']))
        a=analyze_case(r,events,records)
        maximum=a['maximum_event'];key=(maximum['worker'],maximum['request_index'])
        prev=[e for e in events if e['status']=='completed' and e['worker']==key[0] and e['request_index']<key[1]]
        if prev:
            previous=max(prev,key=lambda e:e['request_index']);idx=previous['request_index']
            a['predecessor']=dict(request_index=idx,operation_ms=(previous['finished_ns']-previous['sent_ns'])/1e6,
                overlap_ms=max(0,min(previous['finished_ns'],maximum['sent_ns'])-max(previous['sent_ns'],maximum['planned_ns']))/1e6,
                attribution=next((x for x in a['records'] if (x['worker'],x['request_index'])==(key[0],idx)),None))
        analyses.append(a)
    groups={}
    for r in rows:groups.setdefault((r['scenario'],r['clients'],r['rate'],r['variant']),[]).append(r)
    summary=[]
    def describe(xs):return dict(median=statistics.median(xs),min=min(xs),max=max(xs),runs=xs)
    for (scenario,clients,rate,variant),rs in sorted(groups.items()):
        assert len(rs)==3
        summary.append(dict(scenario=scenario,clients=clients,rate=rate,variant=variant,
            p99_ms=describe([r['response']['p99_ms'] for r in rs]),max_ms=describe([r['response']['max_ms'] for r in rs]),
            ops_s=describe([r['ops_s'] for r in rs]),rejected=sum(r['rejected'] for r in rs),offered=sum(r['attempted'] for r in rs),
            syncs_s=describe([r['syncs_s'] for r in rs])))
    for f in source.iterdir():
        if f.is_file():shutil.copy2(f,out/f.name)
    (out/'analysis.json').write_text(json.dumps(analyses,indent=2)+'\n')
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    lines=['| Scenario | Connections | Offered/s | Mode | Updates/s | p99 ms | Worst ms | Rejected / offered |',
           '|---|---:|---:|---|---:|---:|---:|---:|']
    for s in summary:lines.append(f"| {s['scenario']} | {s['clients']} | {s['rate']} | {s['variant']} | {s['ops_s']['median']:.1f} | {s['p99_ms']['median']:.3f} | {s['max_ms']['max']:.3f} | {s['rejected']} / {s['offered']} |")
    (out/'table.md').write_text('\n'.join(lines)+'\n')
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    variants=sorted({s['variant'] for s in summary});cases=sorted({(s['clients'],s['rate']) for s in summary})
    fig,axes=plt.subplots(3,3,figsize=(15,11),layout='constrained')
    for row,scenario in enumerate(['normal','single','burst']):
        for col,(field,title) in enumerate([('p99_ms','Response p99 (ms)'),('max_ms','Per-run maximum (ms)'),('rejected','Rejected requests / 24k')]):
            ax=axes[row,col]
            for j,v in enumerate(variants):
                g=[next(s for s in summary if (s['scenario'],s['clients'],s['rate'],s['variant'])==(scenario,c,r,v)) for c,r in cases]
                xs=[i+(j-(len(variants)-1)/2)*.32 for i in range(len(cases))]
                label=v.removeprefix('candidate-')
                if field=='rejected':ax.bar(xs,[s[field] for s in g],.3,label=label)
                else:
                    ds=[s[field] for s in g]
                    ax.bar(xs,[d['median'] for d in ds],.3,yerr=[[d['median']-d['min'] for d in ds],[d['max']-d['median'] for d in ds]],capsize=3,label=label)

            if field=='rejected':
                ax.set_ylim(bottom=0,top=max(1,ax.get_ylim()[1]))
                if all(s['rejected']==0 for s in summary if s['scenario']==scenario):ax.text(.5,.5,'0 in all runs',ha='center',transform=ax.transAxes)
            ax.set_xticks(range(len(cases)),[f'{c} conn.\n{r}/s' for c,r in cases]);ax.set_title(scenario+' — '+title);ax.spines[['top','right']].set_visible(False)
    axes[0,0].legend();fig.suptitle('Deferred slow-request diagnostics: same ready-work batching policy')
    fig.supxlabel('100k rows / 256B / 8,000 offered requests. Three runs: latency median and range; all rejection outcomes.\nOff omits lock-timing; profile retains slow requests after sending replies and drains after the workload.',fontsize=10)
    for ext in ['svg','png']:fig.savefig(out/f'observer.{ext}',dpi=140)
    plt.close(fig)
    recorded=[(a,x) for a in analyses for x in a['records']]
    selected=sorted([pair for pair in recorded if pair[0]['scenario']=='normal'],key=lambda pair:pair[1]['server_ms'],reverse=True)[:12]
    if selected:
        fig,ax=plt.subplots(figsize=(14,7),layout='constrained')
        colors=['#475467','#006d77','#a15c00','#8b5fbf','#cc4455']
        labels=['Actual sync call','Coordinator / preceding group wait','Collection interval','Other server work','Post-dispatch send']
        left=[0.0]*len(selected)
        for j,label in enumerate(labels):
            values=[]
            for _,x in selected:
                item=x['items'][0]
                values.append([item['sync'],item['leader_wait'],item['collection'],sum(v for k,v in item.items() if k not in ['sync','leader_wait','collection','send']),item['send']][j])
            ax.barh(range(len(selected)),values,left=left,color=colors[j],label=label);left=[a+b for a,b in zip(left,values)]
        ax.set_yticks(range(len(selected)),[f"r{a['rep']} {a['clients']}c/{a['rate']} {a['scenario']} w{x['worker']} req{x['request_index']}" for a,x in selected]);ax.invert_yaxis()
        ax.set_xlabel('Server wall time (ms); first transaction path in each network batch')
        ax.set_title('Twelve slowest retained normal-load requests — no injected sleep')
        ax.legend(loc='upper center',bbox_to_anchor=(.5,-.12),ncol=2,fontsize=9)
        for ext in ['svg','png']:fig.savefig(out/f'attribution.{ext}',dpi=140)
        plt.close(fig)
    for p in out.glob('*.svg'):p.write_text('\n'.join(line.rstrip() for line in p.read_text().splitlines())+'\n')
    print('Published',len(rows),'cases;',len(recorded),'matched network records')
    print('\n'.join(lines))
if __name__=='__main__':main()
