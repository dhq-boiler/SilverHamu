"""Validate raw request ordering and plot per-trial resource-envelope results."""
import argparse, collections, gzip, hashlib, json, pathlib, shutil, statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

def p99(values):
    values=sorted(values)
    return values[max(0,__import__('math').ceil(len(values)*.99)-1)]/1e6 if values else None

def summary(root):
    result=json.loads((root/'results.json').read_text()); totals=collections.Counter()
    for case in result:
        case['resource_summary']={}
        for phase in ['boot','reopen','rescue']:
            path=root/case['name']/f'{phase}-resources.jsonl'
            if not path.exists():continue
            samples=[json.loads(x) for x in path.read_text().splitlines()]
            assert samples and all(a['time_ns']<=b['time_ns'] for a,b in zip(samples,samples[1:]))
            assert max(x['swap'] for x in samples)==0
            case['resource_summary'][phase]={'samples':len(samples),'cgroup_peak_mib':max(s['peak'] for s in samples)/2**20,
                'sampled_rss_max_mib':max(s['rss'] for s in samples)/2**20,'sampled_file_max_mib':max(s['stat']['file'] for s in samples)/2**20,
                'oom_kills':samples[-1]['events']['oom_kill'],'cpu_usec':samples[-1]['cpu']['usage_usec'],
                'throttled_usec':samples[-1]['cpu']['throttled_usec']}
            if phase=='boot':
                for m in case['measurements']:
                    window=[s for s in samples if m['start_ns']<=s['time_ns']<=m['finish_ns']]
                    if len(window)>1:
                        m['sampled_cpu_cores']=(window[-1]['cpu']['usage_usec']-window[0]['cpu']['usage_usec'])*1000/(window[-1]['time_ns']-window[0]['time_ns'])
                        m['sampled_throttled_ms']=(window[-1]['cpu']['throttled_usec']-window[0]['cpu']['throttled_usec'])/1000
        for m in case['measurements']:
            raw=json.loads((root/case['name']/f"{m['work']}-raw.json").read_text());records=raw['records']
            assert len(records)==m['ops'] and len({x['seq'] for x in records})==m['ops']
            assert all(r['planned']<=r['enqueued']<=r['ended'] for r in records)
            assert all(r['enqueued']<=r['began']<=r['ended'] for r in records if r['status']!='queue_full')
            assert m['completed']+m['queue_rejected']+len(m['errors'])==m['ops']
            m['producer_lag_p99_ms']=p99([r['enqueued']-r['planned'] for r in records])
            m['client_queue_p99_ms']=p99([r['began']-r['enqueued'] for r in records if r['status']=='ok'])
            m['request_p99_ms']=p99([r['ended']-r['began'] for r in records if r['status']=='ok'])
            totals['attempted']+=m['ops'];totals['completed']+=m['completed'];totals['queue_rejected']+=m['queue_rejected'];totals['errors']+=len(m['errors'])
        if case['status']=='failed':
            log=root/case['name']/'boot-server.log'
            case['failure_kind']='engine_resource_budget' if log.exists() and 'resource exhausted: database resource budget' in log.read_text() else 'oom' if case['resource_summary'].get('boot',{}).get('oom_kills',0)>0 else 'unclassified'
        totals[case['status']]+=1;totals['full_checks']+=len(case['verification'])
    return result,dict(totals)

def main():
    p=argparse.ArgumentParser();p.add_argument('--input',type=pathlib.Path,nargs='+',required=True);p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();a.out.mkdir(parents=True,exist_ok=True)
    allcases=[];counts={}
    for root in a.input:
        cases,total=summary(root);counts[root.name]=total
        args=json.loads((root/'environment.json').read_text())['arguments']
        kind='oom' if args.get('recover_memory') else 'load' if args.get('rate',1000)==5000 else 'main'
        for case in cases:case['cohort']=root.name;case['cohort_kind']=kind
        allcases+=cases
        target=a.out/'raw'/root.name;target.mkdir(parents=True,exist_ok=True)
        shutil.copy2(root/'environment.json',target/'environment.json')
        for f in root.rglob('*'):
            if f.is_file() and f.name!='environment.json':
                dest=target/f.relative_to(root);dest.parent.mkdir(parents=True,exist_ok=True)
                with gzip.open(str(dest)+'.gz','wb') as out:out.write(f.read_bytes())
    (a.out/'summary.json').write_text(json.dumps({'counts':counts,'cases':allcases},indent=2))
    cases=[c for c in allcases if c['cohort_kind']=='main']
    groups=collections.defaultdict(list)
    for c in cases:
        for m in c['measurements']:groups[(c['cpus'],c['memory_mib'],m['work'])].append((c,m))
    fig,axes=plt.subplots(1,3,figsize=(14,4.3))
    labels=[];p99=[];rss=[];boot=[];trials=[[],[],[]]
    for (cpu,mem,work),items in sorted(groups.items()):
        if work!='mixed':continue
        labels.append(f'{cpu} CPU\n{mem} MiB');p99.append(statistics.median(m['p99_ms'] for c,m in items));rss.append(statistics.median(c['resource_summary']['boot']['cgroup_peak_mib'] for c,m in items));boot.append(statistics.median(c['boot']['timing']['launch_to_ready_ms'] for c,m in items))
        trials[0].append([m['p99_ms'] for c,m in items]);trials[1].append([c['resource_summary']['boot']['cgroup_peak_mib'] for c,m in items]);trials[2].append([c['boot']['timing']['launch_to_ready_ms'] for c,m in items])
    for ax,values,points,title,ylabel in zip(axes,[p99,rss,boot],trials,['Mixed workload: p99','Server cgroup peak','Container launch to ready'],['ms (scheduled arrival to completion)','MiB (includes observer)','ms (includes Docker orchestration)']):
        ax.bar(range(len(values)),values,color='#3678aa');ax.set_xticks(range(len(values)),labels,fontsize=8);ax.set_title(title);ax.set_ylabel(ylabel);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
        for i,ys in enumerate(points):ax.scatter([i+(j-1)*.12 for j in range(len(ys))],ys,c='#202020',s=12,zorder=3)
        for i,v in enumerate(values):ax.text(i,v,f'{v:.2f}',ha='center',va='bottom',fontsize=8)
        ax.margins(y=.18)
    fig.suptitle('catdb default service: 100,000 rows / 256 B / 1,000 offered requests/s\nMedians of 3 trials; 1,000,000 rows rejected by engine budget in all configurations',fontsize=11)
    fig.tight_layout();fig.savefig(a.out/'comparison.svg');fig.savefig(a.out/'comparison.png',dpi=140);plt.close(fig)
    stress=collections.defaultdict(list)
    for c in allcases:
        if c['cohort_kind']=='load':stress[c['cpus'],c['sample']].append(c)
    if stress:
        fig,axes=plt.subplots(1,3,figsize=(12,4.5));labels=[];vals=[[],[],[]];points=[[],[],[]]
        for (cpu,sample),cs in sorted(stress.items()):
            labels.append(f'{cpu} CPU\nobserver {"on" if sample else "off"}')
            read=[next(m for m in c['measurements'] if m['work']=='read')['p99_ms'] for c in cs]
            mixed=[next(m for m in c['measurements'] if m['work']=='mixed')['p99_ms'] for c in cs]
            rejects=[next(m for m in c['measurements'] if m['work']=='mixed')['queue_rejected']/80 for c in cs]
            for i,items in enumerate([read,mixed,rejects]):vals[i].append(statistics.median(items));points[i].append(items)
        for ax,ys,pts,title,ylabel in zip(axes,vals,points,['Read-only p99','Mixed-workload p99','Mixed-workload queue rejection'],['ms','ms','% of scheduled requests']):
            ax.bar(range(len(ys)),ys,color='#3678aa');ax.set_xticks(range(len(ys)),labels);ax.set_title(title);ax.set_ylabel(ylabel);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
            for i,items in enumerate(pts):ax.scatter([i+(j-1)*.12 for j in range(len(items))],items,color='#202020',s=16,zorder=3)
            ax.margins(y=.2)
        fig.suptitle('100,000 rows / 512 MiB / 5,000 offered requests/s\nBars: median of 3 trials; dots: individual trials; no engine change',fontsize=11)
        fig.tight_layout();fig.savefig(a.out/'load-observer.svg');fig.savefig(a.out/'load-observer.png',dpi=140);plt.close(fig)
    for svg in a.out.glob('*.svg'):
        svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines())+'\n')
    print(json.dumps(counts,indent=2))

if __name__=='__main__':main()
