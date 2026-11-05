"""Completion-independent offered load, with all admission delay retained."""
import gzip,importlib.util,json,math,pathlib,random,sys,time
HERE=pathlib.Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parent));sys.path.insert(0,str(HERE.parent/'commit-tail'))
import runner as base
spec=importlib.util.spec_from_file_location('tail_client',HERE.parent/'commit-tail/client.py')
tail=importlib.util.module_from_spec(spec);spec.loader.exec_module(tail)
from schedule import execute

def percentile(xs,p):
    xs=sorted(xs)
    return xs[max(0,math.ceil(len(xs)*p)-1)] if xs else None

def measure(clients,rep,rate,profile):
    tail.profile=profile
    class Adapter:
        def __init__(self,w):
            self.w=w;self.a=tail.Single();self.maximum=0;self.maximum_response=0
            rng=random.Random(424242+rep*100+w)
            self.keys=[base.key(rng.randrange((base.ROWS-w+clients-1)//clients)*clients+w) for _ in range(base.OPS//clients)]
        def warm(self):
            for k in self.keys[:50]:self.a.get(k)
        def counters(self):return self.a.request(op='benchmark_counters')
        def update(self,event):
            k=self.keys[event['sequence']]
            stamp=rep*1_000_000_000_000+rate*100_000_000+clients*1_000_000+self.w*base.OPS+event['sequence']+1
            event['key_index']=int.from_bytes(k,'big');event['stamp']=stamp
            self.a.put(k,base.value(event['key_index'],stamp));self.a.commit()
        def probe(self,event):
            if not profile:return None
            operation=event['finished_ns']-event['sent_ns'];response=event['finished_ns']-event['planned_ns']
            capture=response>=10_000_000 or operation>self.maximum or response>self.maximum_response
            self.maximum=max(self.maximum,operation);self.maximum_response=max(self.maximum_response,response)
            if not capture:return None
            row={'id':event['id'],'worker':self.w,'sequence':event['sequence'],
                 'operation_ns':operation,'response_ns':response,'request_ns':self.a.last_request_ns,
                 'request_index':self.a.request_index,'ok':True}
            began=time.perf_counter_ns()
            try:
                result=self.a.client.request(op='diagnostic_last');row['diagnostic']=result.get('diagnostic')
                if not result.get('ok') or row['diagnostic'] is None:raise ValueError('missing diagnostic')
                if row['diagnostic']['request_index']!=self.a.request_index:raise ValueError('request mismatch')
            except Exception as error:row['probe_error']=str(error)
            row['probe_ns']=time.perf_counter_ns()-began
            return row
        def close(self):self.a.close()
    result=execute(Adapter,base.OPS,clients,rate)
    events=result['events'];completed=[e for e in events if e['status']=='completed']
    changes=json.loads(pathlib.Path('/data/updated.json').read_text())
    for e in completed:changes[str(e['key_index'])]=e['stamp']
    pathlib.Path('/data/updated.json').write_text(json.dumps(changes))
    before=result['counters']['before'];after=result['counters']['after']
    assert int(after['version'])-int(before['version'])==len(completed)
    syncs=after['sync_attempts']-before['sync_attempts']
    end=max(e['finished_ns'] for e in events);seconds=(end-result['origin_ns'])/1e9
    summary={'engine':'catdb-service','workload':'update','clients':clients,'rep':rep,'rate':rate,
             'rows':base.ROWS,'value_bytes':256,'attempted':base.OPS,'completed':len(completed),
             'rejected':sum(e['status']=='rejected' for e in events),
             'errors':[e for e in events if e['status']=='failed'],'retries':0,'elapsed_s':seconds,
             'ops_s':len(completed)/seconds,'sync_attempts':syncs,'syncs_s':syncs/seconds,
             'updates_per_sync':len(completed)/syncs if syncs else None,
             'events':events,'schedule':{k:v for k,v in result.items() if k not in ('events','probes')}}
    for name,start in [('operation','sent_ns'),('response','planned_ns'),('queue','planned_ns')]:
        xs=[(e['sent_ns'] if name=='queue' else e['finished_ns'])-e[start] for e in completed]
        summary[name]={'p50_ms':percentile(xs,.5)/1e6,'p99_ms':percentile(xs,.99)/1e6,
                       'max_ms':max(xs)/1e6,'above_ms':{str(n):sum(x>n*1e6 for x in xs) for n in (10,20,50)}}
    summary['p99_ms']=summary['response']['p99_ms']
    if profile:summary['profile']={'records':result['probes'],'limit':base.OPS,'dropped':0}
    return summary

if __name__=='__main__':
    action=sys.argv[1]
    if action=='init':base.initialize('catdb-service');result={'initialized':base.ROWS}
    elif action=='verify':result=base.verify('catdb-service')
    else:result=measure(int(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4]),sys.argv[5]=='1')
    print(json.dumps(result))
