"""Request-local timing partition and time-window analysis; no cross-clock subtraction."""
import gzip,importlib.util,json,math,pathlib,sys
spec=importlib.util.spec_from_file_location('aged_metrics',pathlib.Path(__file__).parents[1]/'aged-service/analyze.py')
metrics=importlib.util.module_from_spec(spec);spec.loader.exec_module(metrics)
data,stats,measured=metrics.data,metrics.stats,metrics.measured

def phases(row):
    d=row['diagnostic'];t=d['trace'];q=d['detail'];c=d['costs']
    assert row['status']=='ok' and d['commit_ok'] and len(t)==len(q)==8 and len(c)==2
    assert all(type(x)==int and x>0 for x in t+q) and t==sorted(t)
    assert all(type(x)==int and x>=0 for x in c)
    start=d['finished_ns']-d['dispatch_ns']
    assert q[1]<=t[1]
    stamps=[start,t[0],max(t[0],q[1]),t[1],t[2],t[3],q[2],q[3],t[4],t[5],t[6],t[7],q[5],q[6],d['finished_ns']]
    assert stamps==sorted(stamps),stamps
    names=['entry','wait_before_collection','collection_wait','before_writer','writer_wait','initial_state_wait',
           'validate_and_frame','reserve','write_envelope','sync_envelope','publication',
           'post_publish_to_reply','reply_delivery','response_tail']
    parts={name:(b-a)/1e6 for name,a,b in zip(names,stamps,stamps[1:])}
    assert 0<=sum(c)<=q[3]-q[2]
    return parts

def requests(rows):
    assert rows and len({x['seq'] for x in rows})==len(rows)
    out={}
    for kind in ['read','write']:
        selected=[x for x in rows if x['kind']==kind];ok=[x for x in selected if x['status']=='ok']
        if not selected:continue
        for x in ok:assert x['planned']<=x['enqueued']<=x['began']<=x['ended']
        out[kind]={'offered':len(selected),'ok':len(ok),'queue_refused':sum(x['status']=='queue_full' for x in selected),
            'errors':sum(x['status']=='error' for x in selected),
            'response_ms':stats([(x['ended']-x['planned'])/1e6 for x in ok]),
            'before_call_ms':stats([(x['began']-x['planned'])/1e6 for x in ok]),
            'call_ms':stats([(x['ended']-x['began'])/1e6 for x in ok]),
            'over10ms':sum(x['ended']-x['planned']>=10_000_000 for x in ok),
            'over20ms':sum(x['ended']-x['planned']>=20_000_000 for x in ok)}
    return out

def trial(folder,info):
    result=json.loads((folder/'results.json').read_text())[0];assert result['status']=='passed'
    case=folder/result['name'];mixed=json.loads(data(case/'mixed-raw.json'));read=json.loads(data(case/'read-raw.json'))
    records=mixed['records'];assert len(records)==mixed['ops'] and not mixed['errors'] and not read['errors']
    samples=[json.loads(x) for x in data(case/'boot-resources.jsonl').splitlines()]
    out={**info,'mixed':requests(records),'read_only':requests(read['records']),
        'requests':mixed['ops']+read['ops'],'ops_s':mixed['ops_s'],
        'resources':measured(samples,mixed['start_ns'],mixed['finish_ns']),
        'full_checks':len(result['verification']),'intervals':[]}
    for name,lo,hi in [('first_second',0,1),('remaining',1,float('inf'))]:
        chosen=[x for x in records if lo<=(x['planned']-mixed['start_ns'])/1e9<hi]
        out[name]=requests(chosen)
    for index in range(math.ceil(mixed['ops']/mixed['rate']*10)):
        chosen=[x for x in records if (x['planned']-mixed['start_ns'])//100_000_000==index]
        if chosen:out['intervals'].append({'start_s':index/10,**requests(chosen)})
    writes=[x for x in records if x['kind']=='write' and x['status']=='ok']
    worst=max(writes,key=lambda x:x['ended']-x['planned'])
    out['worst_write']={'seq':worst['seq'],'planned_s':(worst['planned']-mixed['start_ns'])/1e9,
        'response_ms':(worst['ended']-worst['planned'])/1e6,'before_call_ms':(worst['began']-worst['planned'])/1e6,'call_ms':(worst['ended']-worst['began'])/1e6}
    if info['variant']=='diagnostic':
        parts=[phases(x) for x in writes];out['phases_ms']={k:stats([v[k] for v in parts]) for k in parts[0]}
        out['worst_write']['phases_ms']=phases(worst)
        out['worst_write']['dispatch_ms']=worst['diagnostic']['dispatch_ns']/1e6
        out['diagnostic_records']=len(parts)
        out['slow_diagnostics']=[{'seq':x['seq'],'planned_s':(x['planned']-mixed['start_ns'])/1e9,'response_ms':(x['ended']-x['planned'])/1e6,'phases_ms':phases(x)} for x in writes if x['ended']-x['planned']>=10_000_000]
        assert all(len(json.dumps(x['diagnostic']))<2048 for x in writes)
    else:assert not any('diagnostic' in x for x in writes)
    return out

def analyze(root):
    schedule=json.loads((root/'raw/schedule.json').read_text());out=[];hashes=set()
    for info in schedule:
        folder=root/'raw'/info['label'];r=trial(folder, {k:info[k] for k in ['label','stage','rep','variant']})
        assert r['full_checks']==5
        seed=json.loads((folder/'aged-seed-1000000.json').read_text());hashes.add(seed['wal_sha256'])
        assert seed['verification']['sha256']==seed['verification']['expected_sha256'];out.append(r)
    assert len(hashes)==1
    original=pathlib.Path(__file__).resolve().parents[2]/'docs/benchmarks/aged-service-2026-11-06/raw/main-2-inline-1024'
    assert json.loads((original/'aged-seed-1000000.json').read_text())['wal_sha256'] in hashes
    old=trial(original,{'label':'original-19ms','stage':'original','rep':2,'variant':'inline'})
    return {'trials':out,'original':old,'seed_wal_sha256':hashes.pop()}

if __name__=='__main__':
    root=pathlib.Path(sys.argv[1]);result=analyze(root);(root/'summary.json').write_text(json.dumps(result,indent=2))
    for x in result['trials']:print(x['label'],x['mixed']['write']['response_ms'],x['mixed']['write']['over10ms'])
