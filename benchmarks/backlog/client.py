import json
import math
from pathlib import Path
import random
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent.parent))
import runner as base
from schedule import execute

SCENARIOS={'normal':dict(start_ms=1100,duration_ms=0,delay_ms=0,count=0),
           'single':dict(start_ms=1100,duration_ms=10000,delay_ms=100,count=1),
           'burst':dict(start_ms=1100,duration_ms=1000,delay_ms=20,count=10000)}

def metrics(xs):
    xs=sorted(xs)
    return dict(p50_ms=xs[math.ceil(len(xs)*.5)-1]/1e6 if xs else None,
                p99_ms=xs[math.ceil(len(xs)*.99)-1]/1e6 if xs else None,
                max_ms=max(xs)/1e6 if xs else None,
                above_ms={str(n):sum(x>n*1e6 for x in xs) for n in (10,20,50)})

def measure(clients,rep,rate,scenario,limit):
    class Adapter:
        def __init__(self,w):
            self.w=w;self.a=base.Remote()
            rng=random.Random(424242+rep*100+w)
            self.keys=[base.key(rng.randrange((base.ROWS-w+clients-1)//clients)*clients+w) for _ in range(base.OPS//clients)]
        def request(self,**kw):return self.a.client.request(**kw)
        def warm(self):
            for k in self.keys[:50]:
                r=self.request(op='get_latest',key=k.hex());assert r['ok']
        def counters(self):
            r=self.request(op='benchmark_counters');assert r['ok'];self.has_stalls='injected_stalls' in r;return r
        def arm(self):
            if not self.has_stalls:
                assert scenario=='normal';return
            r=self.request(op='benchmark_stall',**SCENARIOS[scenario]);assert r['ok']
        def identity(self,e):return self.keys[e['sequence']]
        def update_many(self,events):
            entries=[]
            for e in events:
                k=self.identity(e)
                stamp=rep*10_000_000_000_000+list(SCENARIOS).index(scenario)*1_000_000_000_000+rate*100_000_000+clients*1_000_000+self.w*base.OPS+e['sequence']+1
                e.update(key_index=int.from_bytes(k,'big'),stamp=stamp)
                entries.append(dict(key=k.hex(),value=base.value(e['key_index'],stamp).hex()))
            if len(entries)==1:
                return [self.request(op='put_commit',**entries[0])]
            result=self.request(op='put_batch_commit',entries=entries)
            if not result.get('ok'):return [result]*len(entries)
            return result['results']
        def close(self):self.a.close()
    raw=execute(Adapter,base.OPS,clients,rate,limit)
    events=raw.pop('events');batches=raw.pop('batches')
    completed=[e for e in events if e['status']=='completed']
    before=raw['counters']['before'];after=raw['counters']['after']
    assert int(after['version'])-int(before['version'])==len(completed)
    injected=after.get('injected_stalls',0)
    assert (injected==0 if scenario=='normal' else injected==1 if scenario=='single' else injected>0)
    timing=after.get('injected_stall_ns',[0,0,0])
    if injected:
        assert timing[0]>0 and timing[1]>=timing[0] and 0<timing[2]<=timing[1]-timing[0]
        if scenario=='single':assert timing[2]==timing[1]-timing[0]
    else:assert timing==[0,0,0]
    changes=json.loads(Path('/data/updated.json').read_text())
    for e in completed:changes[str(e['key_index'])]=e['stamp']
    Path('/data/updated.json').write_text(json.dumps(changes))
    elapsed=(max(e['finished_ns'] for e in events)-raw['origin_ns'])/1e9
    syncs=after['sync_attempts']-before['sync_attempts']
    result=dict(clients=clients,rep=rep,rate=rate,scenario=scenario,rows=base.ROWS,value_bytes=256,
                attempted=base.OPS,completed=len(completed),rejected=sum(e['status']=='rejected' for e in events),
                errors=[e for e in events if e['status']=='failed'],elapsed_s=elapsed,ops_s=len(completed)/elapsed,
                sync_attempts=syncs,syncs_s=syncs/elapsed,updates_per_sync=len(completed)/syncs if syncs else None,
                injected_stalls=injected,events=events,batches=batches,schedule=raw,stall=SCENARIOS[scenario])
    for name,start,end in [('response','planned_ns','finished_ns'),('operation','sent_ns','finished_ns'),('queue','planned_ns','sent_ns')]:
        result[name]=metrics([e[end]-e[start] for e in completed])
    return result

if __name__=='__main__':
    action=sys.argv[1]
    if action=='init':base.initialize('catdb-service');result={'initialized':base.ROWS}
    elif action=='verify':result=base.verify('catdb-service')
    else:result=measure(int(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4]),sys.argv[5],int(sys.argv[6]))
    print(json.dumps(result))
