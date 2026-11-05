"""Match deferred per-connection records to exact network batches."""
import gzip,json,sys
from pathlib import Path

def phases(row,item,sync_start):
    t=item['trace'];d=item['detail']
    if not item['commit_ok']:return None
    assert all(t) and all(d), (t,d)
    assert row['start_ns']<=d[7]<=d[0]<=t[0]<=t[1]<=t[2]<=t[3]<=t[4]<=t[5]<=sync_start<=t[6]<=t[7]<=d[5]<=d[6]<=row['end_ns']<=row['sent_ns']
    assert d[1]<=t[1] and t[3]<=d[2]<=d[3]<=t[4] and t[6]<=d[4]<=t[7]
    ns=dict(entry=d[7]-row['start_ns'],pending_lock=t[0]-d[0],
            leader_wait=max(0,d[1]-t[0]),collection=t[1]-max(d[1],t[0]),
            writer_lock=t[3]-t[2],state_lock=d[2]-t[3],validate_build=d[3]-d[2],reserve=t[4]-d[3],write=t[5]-t[4],
            before_sync=sync_start-t[5],sync=t[6]-sync_start,publication_lock=d[4]-t[6],publication=t[7]-d[4],
            release_to_reply=d[5]-t[7],receive=d[6]-d[5],
            dispatch_end=row['end_ns']-d[6],send=row['sent_ns']-row['end_ns'])
    ns['other']=row['sent_ns']-row['start_ns']-sum(ns.values())
    assert ns['other']>=0
    return {k:v/1e6 for k,v in ns.items()}

def analyze_case(r,events,diagnostics):
    completed=[e for e in events if e['status']=='completed']
    groups={}
    for e in completed:groups.setdefault((e['worker'],e['request_index']),[]).append(e)
    matched=[];seen=set()
    for worker in diagnostics:
        if worker is None:continue
        assert worker['dropped']==0, 'diagnostic loss'
        for row in worker['records']:
            key=(worker['worker'],row['request_index']);assert key not in seen;seen.add(key)
            group=groups[key];assert len(row['items'])==len(group)==len(row['sync_starts'])
            assert row['sent_ns']-row['start_ns']>=worker['threshold_ns']
            operation=group[0]['finished_ns']-group[0]['sent_ns']
            # Server post-send timestamp can follow client receive by a scheduling interval.
            record=dict(worker=key[0],request_index=key[1],batch_size=len(group),server_ms=(row['sent_ns']-row['start_ns'])/1e6,
                        operation_ms=operation/1e6,response_max_ms=max(e['finished_ns']-e['planned_ns'] for e in group)/1e6,
                        client_queue_max_ms=max(e['sent_ns']-e['planned_ns'] for e in group)/1e6,
                        residual_ms=(operation-(row['sent_ns']-row['start_ns']))/1e6,
                        items=[phases(row,item,start) for item,start in zip(row['items'],row['sync_starts'])])
            record['largest_phase']=max(record['items'][0],key=record['items'][0].get)
            matched.append(record)
    maximum=max(completed,key=lambda e:e['finished_ns']-e['planned_ns'])
    maximum_match=next((x for x in matched if (x['worker'],x['request_index'])==(maximum['worker'],maximum['request_index'])),None)
    return dict(variant=r['variant'],rep=r['rep'],clients=r['clients'],rate=r['rate'],scenario=r['scenario'],
                recorded=len(matched),maximum_event=maximum,maximum_attribution=maximum_match,records=matched)

def main():
    p=Path(sys.argv[1]);rows=json.loads((p/'results.json').read_text());analyses=[]
    for r in rows:
        def read(field):return json.load(gzip.open(p/r[field+'_file']))
        a=analyze_case(r,read('events'),read('diagnostics'));analyses.append(a)
        print(r['variant'],r['rep'],r['clients'],r['rate'],r['scenario'],'max',round(r['response']['max_ms'],3),'records',a['recorded'], 'attribution',a['maximum_attribution'])
    (p/'attribution.json').write_text(json.dumps(analyses,indent=2)+'\n')
if __name__=='__main__':main()
