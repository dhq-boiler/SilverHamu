"""The pilot gate is fixed before viewing results; responses end outstanding work."""
import importlib.util,json,pathlib,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
s=importlib.util.spec_from_file_location('aged_metrics',ROOT/'benchmarks/aged-service/analyze.py');metrics=importlib.util.module_from_spec(s);s.loader.exec_module(metrics)
s=importlib.util.spec_from_file_location('aged_requests',ROOT/'benchmarks/aged-tail/analyze.py');timing=importlib.util.module_from_spec(s);s.loader.exec_module(timing)
def recovery(records,lo,hi):
    events=[]
    for x in records:
        if x['kind']=='write' and x['status']!='queue_full':events.extend([(x['enqueued'],1),(x['ended'],-1)])
    count=0;points=[]
    for t,delta in sorted(events):
        count+=delta;assert 0<=count<=64;points.append((t,count))
    assert count==0
    window=[(i,t,n) for i,(t,n) in enumerate(points) if lo<=t<=hi]
    if not window:return None
    peak=max(n for _,_,n in window)
    if peak<=4:return None
    i,t,n=next(x for x in window if x[2]==peak)
    recovered=next((u for u,count in points[i+1:] if count<=4),None)
    return {'peak':peak,'peak_ns':t,'recovered_ns':recovered,'recovery_ms':(recovered-t)/1e6 if recovered else None}
def summarize(root):
    schedule=json.loads((root/'raw/schedule.json').read_text());image=json.loads((root/'raw/image.json').read_text())['id'];out=[];hashes=set()
    for e in schedule:
        folder=root/'raw'/e['label'];r=json.loads((folder/'results.json').read_text())[0];assert r['status']=='passed' and len(r['verification'])==5 and all(x['passed'] for x in r['verification'])
        assert json.loads((folder/'environment.json').read_text())['image']==image
        seed=json.loads((folder/'aged-seed-1000000.json').read_text());assert seed['verification']['sha256']==seed['verification']['expected_sha256'];hashes.add(seed['wal_sha256'])
        assert r['safety_probe']['prior_staged_value_preserved'] and r['safety_probe']['commit_after_rollback']
        case=folder/r['name'];m=json.loads(metrics.data(case/'mixed-raw.json'));read=json.loads(metrics.data(case/'read-raw.json'));samples=[json.loads(x) for x in metrics.data(case/'boot-resources.jsonl').splitlines()]
        assert not m['errors'] and not read['errors'] and m['pin_reader'] and m['old_snapshot_checked_keys']==m['by_kind']['write']['completed']
        assert m['batch_limit']==e['limit'] and m['capacity_per_kind']==64
        assert m['stall_counters']['injected_stalls']==(1 if e['scenario']=='single' else 0)
        for phase in ['boot','reopen']:
            limits=r[phase]['limits'];q,p=map(int,limits['cpu.max'].split());assert q==p and limits['memory.max']=='536870912' and limits['memory.swap.max']=='0'
        for work in [m,read]:
            assert len(work['records'])==work['ops'];assert len({x['seq'] for x in work['records']})==work['ops']
            for w in range(5):
                events=[]
                for x in work['records']:
                    if x['worker']==w and x['status']!='queue_full':events.extend([(x['enqueued'],1),(x['ended'],-1)])
                active=0
                for _,delta in sorted(events):active+=delta;assert 0<=active<=(64 if work['work']=='mixed' and w==0 else 16)
                assert active==0
        row={k:e[k] for k in ['label','scenario','limit']};row.update(mixed=timing.requests(m['records']),read=timing.requests(read['records']),resources=metrics.measured(samples,m['start_ns'],m['finish_ns']),syncs=m['sync_attempts'],updates_per_sync=m['by_kind']['write']['completed']/m['sync_attempts'],ops_s=m['ops_s'],requests=m['ops']+read['ops'],full_checks=5,reopen_ms=r['reopen']['process_ready']['process_ready_ms'],batch_sizes=[b['size'] for b in m['batches']],recovery=recovery(m['records'],m['arm_begin_ns']+500_000_000,m['arm_end_ns']+1_000_000_000))
        for k in row['mixed']:row['mixed'][k]['service_refused']=sum(x['kind']==k and x['status']=='resource_refused' for x in m['records'])
        out.append(row)
    assert len(hashes)==1
    return {'image':image,'seed_wal_sha256':hashes.pop(),'trials':out}
def gate(s):
    assert len(s['trials'])==4
    r={(x['scenario'],x['limit']):x for x in s['trials']};bn,cn,bs,cs=[r[k] for k in [('normal',1),('normal',8),('single',1),('single',8)]]
    latency=lambda x,k,stat='p99':x['mixed'][k]['response_ms'][stat]
    checks={'normal_read_p99':latency(cn,'read')<=max(latency(bn,'read')*1.25,latency(bn,'read')+.2),'normal_write_p99':latency(cn,'write')<=max(latency(bn,'write')*1.25,latency(bn,'write')+1),'single_syncs':cs['syncs']<bs['syncs'],'single_write_p99':latency(cs,'write')<=latency(bs,'write')*1.1,'single_write_max':latency(cs,'write','max')<=latency(bs,'write','max')*1.1}
    for scenario in ['normal','single']:
        b,c=r[(scenario,1)],r[(scenario,8)];checks[scenario+'_rss']=c['resources']['rss_peak_mib']<=b['resources']['rss_peak_mib']*1.05
        checks[scenario+'_refusals']=sum(v['queue_refused']+v.get('service_refused',0) for v in c['mixed'].values())<=sum(v['queue_refused']+v.get('service_refused',0) for v in b['mixed'].values())
    checks['single_recovery']=bool(bs['recovery'] and cs['recovery'] and bs['recovery']['recovery_ms'] is not None and cs['recovery']['recovery_ms'] is not None and cs['recovery']['recovery_ms']<=bs['recovery']['recovery_ms']*.9)
    return {'checks':checks,'proceed':all(checks.values())}
if __name__=='__main__':
    root=pathlib.Path(sys.argv[1]);s=summarize(root);s['gate']=gate(s);(root/'summary.json').write_text(json.dumps(s,indent=2));print(json.dumps(s['gate'],indent=2))
