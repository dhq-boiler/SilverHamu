"""Reject unmatched events; report observed calls, not physical disk service time."""
import argparse,gzip,json,pathlib,statistics
def inspect(d):
    bounds=d['bounds'];events=d['events'];fds=d['fds'];sizes={v['path']:v['size'] for v in fds.values()}
    groups=[[] for _ in bounds];total={};extension=0;rewinds=0;last={};sync_requests=set();checkpoint_requests=set()
    for e in sorted(events,key=lambda e:e[7]):
        r,k,fd,off,n,result,err,start,end=e
        assert 0<=r<len(bounds) and str(fd) in fds and k in (1,2,3,4,5)
        assert bounds[r][0]<=start<=end<=bounds[r][2] and err==0 and result>=0
        groups[r].append(e);path=fds[str(fd)]['path']
        item=total.setdefault(path,{'sync_count':0,'sync_ns':0,'write_count':0,'write_bytes':0,'write_ns':0,'truncate_count':0})
        if k in (1,2):item['sync_count']+=1;item['sync_ns']+=end-start;sync_requests.add(r)
        elif k in (3,4):
            assert result==n and off>=0
            item['write_count']+=1;item['write_bytes']+=result;item['write_ns']+=end-start
            extension+=max(0,off+result-sizes[path]);sizes[path]=max(sizes[path],off+result)
            if path.endswith('-wal') and off==0 and last.get(path,-1)>0:rewinds+=1
            last[path]=off
            if path=='sqlite.db':checkpoint_requests.add(r)
        else:sizes[path]=off;item['truncate_count']+=1
    phases=[]
    for i,(start,mid,end) in enumerate(bounds):
        assert start<=mid<=end and end-start==d['samples_ns'][i]
        g=groups[i]
        assert all(a[8]<=b[7] for a,b in zip(g,g[1:])), 'overlapping or nested observed calls'
        sync=sum(e[8]-e[7] for e in g if e[1] in (1,2));write=sum(e[8]-e[7] for e in g if e[1] in (3,4));other=sum(e[8]-e[7] for e in g if e[1]==5)
        assert sync+write+other<=end-start
        phases.append({'prepare_ns':mid-start,'commit_ns':end-mid,'sync_ns':sync,'write_ns':write,'truncate_ns':other,'residual_ns':end-start-sync-write-other})
    if d['diagnostic']:
        assert len(sync_requests)==len(bounds), 'missing durable sync in a request'
        for path,size in sizes.items():assert size==d['final_sizes'][path], ('unobserved size change',path)
        if d['engine']=='catdb-embedded':assert sum(v['sync_count'] for v in total.values())==len(bounds)
    else:assert not events
    return {'files':total,'extension_bytes':extension,'wal_rewind_observations':rewinds,'db_write_requests':len(checkpoint_requests),'mean_phases_ns':{k:statistics.mean(p[k] for p in phases) for k in phases[0]},'phases':phases}
def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);a=p.parse_args();env=json.loads((a.root/'environment.json').read_text());rows=[];hashes={}
    assert len(env['runs'])==env['reps']*6
    for run in env['runs']:
        assert run['status']=='passed'
        with gzip.open(a.root/run['file'],'rt') as f:d=json.load(f)
        assert len(d['samples_ns'])==env['ops'] and d['checks']==['initial','old','latest','reopen']
        assert hashes.setdefault(run['rep'],d['hashes'])==d['hashes']
        result=inspect(d);result.pop('phases');ns=sorted(d['samples_ns'])
        rows.append({**run,**result,'updates_s':env['ops']/d['elapsed_s'],'p99_ms':ns[(len(ns)*99+99)//100-1]/1e6,'max_ms':max(ns)/1e6})
    (a.root/'summary.json').write_text(json.dumps(rows,indent=2))
    for r in rows:print(r['variant'],round(r['updates_s'],2),round(r['p99_ms'],3),'rewinds',r['wal_rewind_observations'],'DB writes',r['db_write_requests'],'files',r['files'])
if __name__=='__main__':main()
