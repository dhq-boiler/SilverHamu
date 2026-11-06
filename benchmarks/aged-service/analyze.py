"""Keep failed pilot outcomes separate from successful latency samples."""
import gzip,json,math,pathlib,statistics,sys

def data(path):
    p=path if path.exists() else path.with_suffix(path.suffix+'.gz')
    raw=p.read_bytes();return gzip.decompress(raw).decode() if p.suffix=='.gz' else raw.decode()

def stats(a):
    a=sorted(a)
    return {'count':len(a),'median':statistics.median(a) if a else None,'p99':a[math.ceil(.99*len(a))-1] if a else None,'max':max(a) if a else None}

def measured(rows,start,end):
    chosen=[x for x in rows if start<=x['time_ns']<=end]
    assert len(chosen)>=2,'insufficient OS samples'
    first,last=chosen[0],chosen[-1];span=(last['time_ns']-first['time_ns'])/1e9
    return {'sample_count':len(chosen),'sampled_span_s':span,
        'rss_peak_mib':max(x['rss'] for x in chosen)/2**20,
        'cgroup_peak_mib':max(x['memory'] for x in chosen)/2**20,
        'file_cache_peak_mib':max(x['stat']['file'] for x in chosen)/2**20,
        'anon_peak_mib':max(x['stat']['anon'] for x in chosen)/2**20,
        'swap_peak_mib':max(x['swap'] for x in chosen)/2**20,
        'cpu_core_percent':(last['cpu']['usage_usec']-first['cpu']['usage_usec'])/1e6/span*100,
        'throttled_usec':last['cpu']['throttled_usec']-first['cpu']['throttled_usec']}

def summarize(raw):
    schedule=json.loads((raw/'schedule.json').read_text());out=[];failed=set();hashes=set()
    for entry in schedule:
        condition=(entry['variant'],entry['memory_mib']);assert condition not in failed,'failed condition was repeated'
        folder=raw/entry['label'];result=json.loads((folder/'results.json').read_text())[0];case=folder/result['name']
        seed=json.loads((folder/'aged-seed-1000000.json').read_text())
        assert seed['updated_rows']==1000000 and not seed['checkpoint_after_aging']
        assert seed['verification']['sha256']==seed['verification']['expected_sha256'];hashes.add(seed['wal_sha256'])
        row={k:entry[k] for k in ['label','stage','rep','variant','memory_mib','status','oom_kills']}
        samples=[json.loads(x) for x in data(case/'boot-resources.jsonl').splitlines()]
        row['whole_boot_rss_peak_mib']=max(x['rss'] for x in samples)/2**20
        row['whole_boot_cgroup_peak_mib']=max(x['memory'] for x in samples)/2**20
        row['verification_count']=len(result['verification']);row['workloads']={}
        if row['status']!='passed':
            assert row['oom_kills']>0;failed.add(condition);row['failure']=result['error'];out.append(row);continue
        assert row['oom_kills']==0 and len(result['verification'])==5 and all(v['passed'] for v in result['verification'])
        assert result['safety_probe']=={'old_generation_updates_checked':8,'staged_refusal':'resource_exhausted','prior_staged_value_preserved':True,'commit_after_rollback':True}
        row['boot_ready_ms']=result['boot']['process_ready']['process_ready_ms'];row['reopen_ready_ms']=result['reopen']['process_ready']['process_ready_ms']
        row['safety_probe']=result['safety_probe']
        for phase in ['read','mixed']:
            m=json.loads(data(case/(phase+'-raw.json')));records=m['records'];assert len(records)==m['ops']
            assert not m['pin_reader'] and not m['errors']
            assert len({x['seq'] for x in records})==len(records)
            perf={'ops':m['ops'],'completed':m['completed'],'queue_rejected':m['queue_rejected'],'ops_s':m['ops_s'],'by_kind':{}}
            for kind in ['read','write']:
                req=[x for x in records if x['kind']==kind];ok=[x for x in req if x['status']=='ok']
                if req:
                    assert all(x['planned']<=x['enqueued']<=x['began']<=x['ended'] for x in ok)
                    perf['by_kind'][kind]={'response_ms':stats([(x['ended']-x['planned'])/1e6 for x in ok]),'client_queue_ms':stats([(x['began']-x['planned'])/1e6 for x in ok]),'rejected':sum(x['status']=='queue_full' for x in req)}
            perf['resources']=measured(samples,m['start_ns'],m['finish_ns']);row['workloads'][phase]=perf
        out.append(row)
    assert len(hashes)==1,('seed WALs must match across variants',hashes)
    return {'trials':out,'seed_wal_sha256':list(hashes)[0], 'failed_conditions':sorted(failed),
        'requests':sum(w['ops'] for x in out for w in x['workloads'].values()),
        'full_checks':sum(x['verification_count'] for x in out),
        'seed_full_checks':len(out)}

if __name__=='__main__':
    root=pathlib.Path(sys.argv[1]);result=summarize(root/'raw');(root/'summary.json').write_text(json.dumps(result,indent=2))
    for x in result['trials']:
        print(x['label'],x['status'],{k:v['by_kind'] for k,v in x['workloads'].items()})
