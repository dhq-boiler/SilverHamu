"""Validate independent capacity, allocator, RSS and recovery observations."""
import argparse,collections,gzip,json,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
p=argparse.ArgumentParser();p.add_argument('--pilot',type=pathlib.Path,required=True);p.add_argument('--main',type=pathlib.Path,required=True);p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();a.out.mkdir(parents=True,exist_ok=True)
cases=[];checks=0;outcomes=collections.Counter()
for kind,root,expected in [('pilot',a.pilot,4),('main',a.main,12)]:
    rows=json.loads((root/'environment.json').read_text())['args']['rows'];entries=json.loads((root/'results.json').read_text());assert len(entries)==expected
    for c in entries:
        assert c['status']=='passed';case={'cohort':kind,'rows':rows,**c,'events':{},'resources':{}}
        for phase,state in c['phases'].items():
            f=root/c['name']/(phase+'.jsonl');es=[json.loads(l) for l in f.read_text().splitlines()];case['events'][phase]=es
            samples=[json.loads(l) for l in (f.parent/(phase+'-resources.jsonl')).read_text().splitlines()]
            assert all(x['swap']==0 for x in samples)
            assert all(x['time_ns']<=y['time_ns'] for x,y in zip(samples,samples[1:]))
            assert not state['exit']['timed_out']
            if state['success']:assert any(e['event']=='verified' and e['rows']==rows for e in es)
            else:assert phase=='aged512' and state['oom_kills']>0
            checks+=len(state['checks']);outcomes[f'{kind}:{phase}:{"passed" if state["success"] else "oom"}']+=1
            case['resources'][phase]={'peak_bytes':max(x['peak'] for x in samples),'rss_peak_bytes':max(x['rss'] for x in samples),'file_peak_bytes':max(x['stat']['file'] for x in samples),'anon_peak_bytes':max(x['stat']['anon'] for x in samples),'throttled_us':samples[-1]['cpu']['throttled_usec']}
            for e in es:
                if e['event']=='snapshot' and e['metrics'] is not None:
                    m=e['metrics'];assert m['records']==rows and m['capacity_slots']>=m['versions']>=rows
                    assert m['value_bytes']==m['versions']*256
                    assert m['version_capacity_bytes']==m['capacity_slots']*m['version_size']
                    assert m['unused_version_bytes']==(m['capacity_slots']-m['versions'])*m['version_size']
                    assert e['logical_retained_bytes']==rows*208+m['versions']*320
            if phase=='aging':
                assert es[-1]['event']=='complete'
                snap={e['phase']:e for e in es if e['event']=='snapshot'}
                assert snap['before-trim']['metrics']==snap['after-trim']['metrics']
                if c['counted']:
                    assert snap['before-trim']['live_requested_bytes']==snap['after-trim']['live_requested_bytes']
                    if c['pattern']=='spread':
                        for phase_name in ['initial','spread-25','spread-50','spread-100']:
                            assert snap[phase_name]['metrics']['versions']==rows
                    else:
                        assert snap['hot-held']['metrics']['versions']==rows+16*(rows//100)
                        assert snap['hot-drained']['metrics']['versions']==rows
                        assert snap['hot-held']['metrics']['capacity_slots']==snap['hot-drained']['metrics']['capacity_slots']
                for e in es:
                    if e['event']=='latency':assert e['read']['count']==1000 and e['write']['count']==400
        cases.append(case)
    for f in root.rglob('*'):
        if f.is_file():
            target=a.out/'raw'/kind/f.relative_to(root);target.parent.mkdir(parents=True,exist_ok=True)
            if f.name=='environment.json':target.write_bytes(f.read_bytes())
            else:
                with gzip.open(str(target)+'.gz','wb') as g:g.write(f.read_bytes())
(a.out/'summary.json').write_text(json.dumps({'checks':checks,'outcomes':dict(outcomes),'cases':cases},indent=2),encoding='utf8')
main=[c for c in cases if c['cohort']=='main'];med=statistics.median;M=2**20
def snaps(pattern,phase):return [next(e for e in c['events']['aging'] if e['event']=='snapshot' and e['phase']==phase) for c in main if c['counted'] and c['pattern']==pattern]
table=['| Pattern / stage | Versions | Capacity slots | Unused version MiB | Live requested MiB | RSS MiB | Logical retained MiB |','|:---|---:|---:|---:|---:|---:|---:|']
for pattern,phases in [('spread',['initial','spread-25','spread-50','spread-100','after-trim']),('hot',['initial','hot-held','hot-released','hot-drained','before-trim','after-trim'])]:
    for phase in phases:
        ss=snaps(pattern,phase)
        values=[med(s['metrics']['versions'] for s in ss),med(s['metrics']['capacity_slots'] for s in ss),med(s['metrics']['unused_version_bytes']/M for s in ss),med(s['live_requested_bytes']/M for s in ss),med(s['rss_bytes']/M for s in ss),med(s['logical_retained_bytes']/M for s in ss)]
        table.append(f'| {pattern} / {phase} | {values[0]:.0f} | {values[1]:.0f} | '+' | '.join(f'{v:.3f}' for v in values[2:])+' |')
(a.out/'memory-table.md').write_text('\n'.join(table)+'\n',encoding='utf8')
fig,axes=plt.subplots(1,3,figsize=(15,4.7))
phases=['initial','spread-25','spread-50','spread-100'];x=range(4)
axes[0].plot(x,[med(s['metrics']['unused_version_bytes']/M for s in snaps('spread',ph)) for ph in phases],marker='o',color='#b86824');axes[0].set_xticks(x,['Initial','25%','50%','100%']);axes[0].set_title('Spread: unused history capacity');axes[0].set_ylabel('MiB still allocated; GC drained')
for ax,pattern,phases,labels in [(axes[1],'spread',phases,['Initial','25%','50%','100%']),(axes[2],'hot',['hot-held','hot-drained','before-trim','after-trim'],['Old held','GC drained','Before trim','After trim'])]:
    for field,color,label in [('live_requested_bytes','#2979b5','Rust live requested'),('rss_bytes','#b86824','Process RSS')]:
        values=[med(s[field]/M for s in snaps(pattern,ph)) for ph in phases];ax.plot(range(4),values,marker='o',color=color,label=label)
        for i,ph in enumerate(phases):ax.scatter([i]*3,[s[field]/M for s in snaps(pattern,ph)],color=color,s=12)
    ax.set_xticks(range(4),labels,fontsize=8);ax.set_title('Spread: live allocation vs RSS' if pattern=='spread' else 'Hot 1%: release and diagnostic trim');ax.set_ylabel('MiB; nonzero y-axis');ax.legend(fontsize=8)
for ax in axes:ax.grid(axis='y',alpha=.25);ax.set_axisbelow(True)
axes[0].set_ylim(bottom=0)
fig.suptitle('1,000,000 rows / 256 B / 1 CPU / 1 GiB — diagnostic only\n3 trials; medians; counted build; persistence and GC unchanged',fontsize=11);fig.tight_layout();fig.savefig(a.out/'capacity.svg');fig.savefig(a.out/'capacity.png',dpi=140);plt.close(fig)
lat_table=['| Pattern / stage | Plain p99 ms | Observed p99 ms | Plain trial p99 ms | Observed trial p99 ms |','|:---|---:|---:|:---|:---|']
fig,ax=plt.subplots(figsize=(10,4.5));labels=[]
for i,(pattern,phase) in enumerate([('spread','spread-25'),('spread','spread-50'),('spread','spread-100'),('hot','hot-drained')]):
    values=[];labels.append(phase)
    for j,counted in enumerate([False,True]):
        ys=[next(e for e in c['events']['aging'] if e['event']=='latency' and e['phase']==phase)['write']['p99_ns']/1e6 for c in main if c['pattern']==pattern and c['counted']==counted];values.append(ys)
        pos=i+(j-.5)*.32;ax.bar(pos,med(ys),width=.3,color=['#888888','#2979b5'][j],label=['Plain','Observed'][j] if i==0 else None);ax.scatter([pos+(k-1)*.04 for k in range(3)],ys,color='black',s=16,zorder=3)
    lat_table.append(f'| {phase} | {med(values[0]):.3f} | {med(values[1]):.3f} | '+ ' | '.join(', '.join(f'{v:.3f}' for v in ys) for ys in values)+' |')
ax.set_xticks(range(4),labels);ax.set_ylabel('Embedded commit p99 ms; 400 commits per window');ax.set_title('Observation comparison: medians and individual trial p99\nClosed-loop 4 writers; not the service benchmark');ax.legend();ax.grid(axis='y',alpha=.25);ax.set_axisbelow(True);fig.tight_layout();fig.savefig(a.out/'observer.svg');fig.savefig(a.out/'observer.png',dpi=140);plt.close(fig)
(a.out/'latency-table.md').write_text('\n'.join(lat_table)+'\n',encoding='utf8')
for svg in a.out.glob('*.svg'):svg.write_text('\n'.join(l.rstrip() for l in svg.read_text(encoding='utf8').splitlines())+'\n',encoding='utf8')
print('full_checks',checks);print('\n'.join(table));print('\n'.join(lat_table));print(json.dumps(dict(outcomes),indent=2))
