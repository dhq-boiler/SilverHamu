"""Validate same-clock retirement diagnostics; never interpret missing phases as zero cost."""
import gzip, json, math, pathlib, statistics, sys

def load(p):
    return json.loads(gzip.decompress(p.read_bytes()) if p.suffix=='.gz' else p.read_bytes())

def distribution(values):
    a=sorted(values)
    return {'count':len(a),'median':statistics.median(a) if a else None,
            'p99':a[max(0,math.ceil(.99*len(a))-1)] if a else None,'max':max(a) if a else None}

def phases(record):
    d=record['diagnostic'];a=d['retirement'];t=d['trace'];q=d['detail']
    assert record['status']=='ok' and d['commit_ok']
    assert isinstance(a,list) and len(a)==6 and all(type(x)==int and x>=0 for x in a)
    assert len(t)==8 and len(q)==8 and all(x>0 for x in t+q)
    assert t==sorted(t)
    order=[t[7],a[0],a[1],a[2],a[5],q[5],q[6],d['finished_ns']]
    assert all(x>0 for x in order) and order==sorted(order),order
    assert a[3]<=128 and ((a[3]==0)==(a[4]==0))
    return {'publish_to_free':a[0]-t[7], 'free':a[1]-a[0],
            'free_to_writer_release':a[2]-a[1], 'writer_to_persist_return':a[5]-a[2],
            'persist_to_reply_ready':q[5]-a[5], 'reply_ready_to_received':q[6]-q[5],
            'received_to_dispatch_end':d['finished_ns']-q[6],
            'sync_envelope':t[6]-t[5], 'publication':t[7]-t[6],
            'writer_wait':t[3]-t[2], 'collection':t[1]-q[1]}

def analyze(root):
    cases=[]
    for case in sorted(root.glob('pilot-*')):
        results=load(case/'results.json');assert len(results)==1 and results[0]['status']=='passed'
        result=results[0];assert len(result['verification'])==4 and all(v['passed'] for v in result['verification'])
        rawpath=case/result['name']/'mixed-raw.json'
        raw=load(rawpath if rawpath.exists() else rawpath.with_suffix('.json.gz'))
        records=raw['records'];assert len(records)==raw['ops']
        writes=[x for x in records if x['kind']=='write' and x['status']=='ok']
        item={'case':case.name,'mode':case.name.split('-')[-1], 'writes':len(writes),
              'errors':len(raw['errors']),'rejected':raw['queue_rejected'],
              'write_ms':distribution([(x['ended']-x['planned'])/1e6 for x in writes]),
              'read_ms':distribution([(x['ended']-x['planned'])/1e6 for x in records if x['kind']=='read' and x['status']=='ok']),
              'ops_s':raw['ops_s']}
        if item['mode']=='on':
            parts=[phases(x) for x in writes]
            item['phases_us']={k:distribution([v[k]/1000 for v in parts]) for k in parts[0]}
            groups={}
            for x in writes:
                a=x['diagnostic']['retirement'];key=a[0]
                if key in groups:assert groups[key]==a
                groups[key]=a
            item['groups']=len(groups);item['eligible_groups']=sum(a[3]>0 for a in groups.values())
            item['retired_versions']=sum(a[3] for a in groups.values());item['retired_bytes']=sum(a[4] for a in groups.values())
            item['eligible_free_us']=distribution([(a[1]-a[0])/1000 for a in groups.values() if a[3]])
            worst=max(writes,key=lambda x:x['ended']-x['planned'])
            item['worst_write']={'seq':worst['seq'],'total_ms':(worst['ended']-worst['planned'])/1e6,
                'client_queue_ms':(worst['began']-worst['planned'])/1e6,
                'dispatch_ms':worst['diagnostic']['dispatch_ns']/1e6,
                'phases_us':{k:v/1000 for k,v in phases(worst).items()}}
        else:assert not any('diagnostic' in x for x in writes)
        resource=case/result['name']/'boot-resources.jsonl'
        content=(gzip.decompress(resource.with_suffix('.jsonl.gz').read_bytes()).decode() if not resource.exists() else resource.read_text())
        samples=[json.loads(line) for line in content.splitlines()]
        item['whole_boot_peak_mib']={k:max(x.get(k,0) for x in samples)/1048576 for k in ['rss','memory','swap']}
        assert not any(x['events']['oom_kill'] for x in samples)
        cases.append(item)
    assert cases
    return cases

if __name__=='__main__':
    root=pathlib.Path(sys.argv[1]);result=analyze(root)
    (root/'summary.json').write_text(json.dumps(result,indent=2),encoding='utf8')
    print(json.dumps(result,indent=2))
