"""A sync is partitioned once; blocked time is not physical disk latency."""
import importlib.util,json,pathlib,re,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('sync_parser',ROOT/'benchmarks/sync-os/analyze.py');base=importlib.util.module_from_spec(spec);spec.loader.exec_module(base)

def partition(events,start,end,tids,wal_fd):
    active={};rows=[]
    def move(tid,t,state=None):
        a=active.get(tid)
        if a is None:return
        assert t>=a['last'],'reversed kernel clock'
        a[a['state']]+=t-a['last'];a['last']=t
        if state:a['state']=state
    for e in events:
        body=e['body'];t=e['ns'];pid=e['pid']
        enter=re.match(r'sys_(fsync|fdatasync)\(fd: (?:0x)?([0-9a-f]+)\)',body)
        leave=re.match(r'sys_(fsync|fdatasync) -> (.+)',body)
        if enter and pid in tids and start<=t<=end:
            assert int(enter[2],16)==wal_fd,'unexpected synchronization fd'
            assert pid not in active,'nested sync'
            active[pid]={'tid':pid,'syscall':enter[1],'start_ns':t,'last':t,'state':'scheduled_ns','scheduled_ns':0,'runnable_ns':0,'blocked_ns':0}
        elif leave and pid in active:
            a=active[pid];assert leave[1]==a['syscall'],'mismatched sync return'
            move(pid,t);a=active.pop(pid);a['end_ns']=t;a['duration_ns']=t-a['start_ns'];a['return']=leave[2]
            assert int(a['return'],16)==0,'failed synchronization'
            assert a['duration_ns']==a['scheduled_ns']+a['runnable_ns']+a['blocked_ns']
            a.pop('last');a.pop('state');rows.append(a)
        elif body.startswith('sched_switch:'):
            m=re.search(r'prev_pid=(\d+).*prev_state=(\S+).*next_pid=(\d+)',body);assert m,'malformed switch'
            prev,state,nxt=m.groups();prev=int(prev);nxt=int(nxt)
            if prev in active:
                assert active[prev]['state']=='scheduled_ns','duplicate or missing switch'
                move(prev,t,'runnable_ns' if state.startswith('R') else 'blocked_ns')
            if nxt in active:
                assert active[nxt]['state']=='runnable_ns','scheduled without wakeup or switch-out'
                move(nxt,t,'scheduled_ns')
        elif body.startswith('sched_wakeup:'):
            tid=int(re.search(r' pid=(\d+)',body)[1])
            if tid in active and active[tid]['state']=='blocked_ns':move(tid,t,'runnable_ns')
    assert not active,'incomplete synchronization'
    return rows

def validate(meta,measurement,events):
    assert meta['cleaned'] and meta['clock']=='mono'
    assert meta['start_ns']<=measurement['start_ns']<=measurement['finish_ns']<=meta['end_ns'],'clock/capture boundary mismatch'
    for value in meta['stats'].values():
        for field in ['overrun','commit overrun','dropped events']:
            found=re.search(r'^'+field+r': (\d+)',value,re.M);assert found and int(found[1])==0,'trace loss or absent loss counter'
    rows=partition(events,measurement['start_ns'],measurement['finish_ns'],set(meta['tids']),meta['wal_fd'])
    assert len(rows)==measurement['sync_attempts'],'sync-count mismatch'
    return rows

def check(folder):
    folder=pathlib.Path(folder);r=json.loads((folder/'results.json').read_text())[0];assert r['status']=='passed'
    out={}
    for m in r['measurements']:
        p=folder/r['name']/(m['work']+'-kernel');meta=json.loads((p/'metadata.json').read_text())
        rows=validate(meta,m,base.parse(p/'trace.txt.gz'));out[m['work']]={'syncs':rows,'capture_bytes':meta['uncompressed_bytes'],'kernel_buffer_bytes':meta['trace_buffer_bytes']}
    (folder/'kernel-analysis.json').write_text(json.dumps(out,indent=2))
    print(json.dumps({k:{'syncs':len(v['syncs']),'capture_bytes':v['capture_bytes']} for k,v in out.items()}))
    return out
if __name__=='__main__':check(sys.argv[1])
