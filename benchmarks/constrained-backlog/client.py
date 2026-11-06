"""Reuse the existing API, with equal queued-plus-active credits in both policies."""
import argparse,importlib.util,json,os,pathlib,queue,random,runpy,threading,time
ROOT=pathlib.Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('resource_base',ROOT/'resource-envelope/client.py');base=importlib.util.module_from_spec(spec);spec.loader.exec_module(base)

def drain(q,first,limit,identity):
    group=[first];keys={identity(first)};carry=None;ended=False
    while len(group)<limit:
        try:item=q.get_nowait()
        except queue.Empty:break
        if item is None:ended=True;break
        if identity(item) in keys:carry=item;break
        keys.add(identity(item));group.append(item)
    return group,carry,ended

def measure(work,ops,rate,capacity,pin_reader=True,before_start=None,batch_limit=1,scenario='normal',setup_seconds=3):
    assert ops>0 and rate>0 and capacity>=4 and capacity%4==0 and batch_limit in [1,8] and scenario in ['normal','single'] and (ops+4)//5<=base.ROWS
    initial=json.loads((base.DATA/'updated.json').read_text());pin=base.connect() if pin_reader else None
    if pin:base.require(pin.request(op='begin_read'))
    kinds=['read']*4 if work=='read' else ['read']+['write']*4
    capacities=[capacity//4]*4 if work=='read' else [capacity]+[capacity//4]*4
    queues=[queue.Queue(c) for c in capacities];credits=[threading.BoundedSemaphore(c) for c in capacities]
    ready=threading.Barrier(len(kinds)+1);start=threading.Event();records=[];changes={};batches=[];fatal=[];lock=threading.Lock()
    def worker(w,kind):
        c=None
        try:
            c=base.connect();ready.wait(timeout=30)
            if not start.wait(timeout=60):raise TimeoutError('start gate')
            carry=None;ended=False
            while not ended:
                first=carry if carry is not None else queues[w].get();carry=None
                if first is None:break
                group,carry,ended=drain(queues[w],first,batch_limit if kind=='write' else 1,lambda x:x[1])
                began=time.monotonic_ns();outcomes=[]
                try:
                    if kind=='write':
                        entries=[dict(key=base.key(k).hex(),value=base.value(k,seq+1).hex()) for seq,k,_,_ in group]
                        if len(entries)==1:outcomes=[c.request(op='put_commit',**entries[0])]
                        else:
                            reply=c.request(op='put_batch_commit',entries=entries)
                            outcomes=reply.get('results',[]) if reply.get('ok') else [reply]*len(entries)
                        if len(outcomes)!=len(group):raise RuntimeError('missing independent outcome')
                    else:
                        seq,k,_,_=first;b=bytes.fromhex(base.require(c.request(op='get_latest',key=base.key(k).hex()))['value'])
                        assert len(b)==256 and b[:8]==base.key(k) and b[16:]==b'x'*240
                        stamp=int.from_bytes(b[8:16],'big');assert stamp==initial.get(str(k),0) or (work=='mixed' and (stamp-1)%5==0 and (stamp-1)//5==k)
                        outcomes=[{'ok':True}]
                except Exception as e:outcomes=[{'ok':False,'error':str(e)}]*len(group)
                ended_ns=time.monotonic_ns()
                with lock:
                    if kind=='write':batches.append({'worker':w,'size':len(group),'began':began,'ended':ended_ns})
                    for (seq,k,planned,enqueued),reply in zip(group,outcomes):
                        rec=dict(seq=seq,kind=kind,worker=w,planned=planned,enqueued=enqueued,began=began,ended=ended_ns,status='ok' if reply.get('ok') else 'resource_refused' if reply.get('error')=='resource_exhausted' else 'error')
                        if reply.get('ok') and kind=='write':changes[str(k)]=seq+1
                        if not reply.get('ok'):rec['error']=reply.get('error','missing outcome')
                        records.append(rec);credits[w].release()
        except BaseException as e:fatal.append(repr(e));ready.abort()
        finally:
            if c:c.close()
    threads=[threading.Thread(target=worker,args=(i,k),daemon=True) for i,k in enumerate(kinds)]
    for t in threads:t.start()
    try:
        ready.wait(timeout=35)
        if before_start:before_start()
        time.sleep(setup_seconds)
        with base.connect() as c:
            before=base.require(c.request(op='benchmark_counters'))
            inject=work=='mixed' and scenario=='single'
            arm_begin=time.monotonic_ns();base.require(c.request(op='benchmark_stall',start_ms=500,duration_ms=10000 if inject else 0,delay_ms=100 if inject else 0,count=1 if inject else 0));arm_end=time.monotonic_ns()
        t0=time.monotonic_ns();start.set();rng=random.Random(739)
        for seq in range(ops):
            planned=t0+int(seq*1e9/rate)
            delay=(planned-time.monotonic_ns())/1e9
            if delay>0:time.sleep(delay)
            kind='write' if work=='mixed' and seq%5==0 else 'read';k=seq//5 if kind=='write' else rng.randrange(base.ROWS)
            w=1+k%4 if kind=='write' else 0 if work=='mixed' else seq%4
            enqueued=time.monotonic_ns()
            if credits[w].acquire(blocking=False):queues[w].put_nowait((seq,k,planned,enqueued))
            else:records.append(dict(seq=seq,kind=kind,worker=w,planned=planned,enqueued=enqueued,ended=time.monotonic_ns(),status='queue_full'))
        for q in queues:q.put(None,timeout=60)
        for t in threads:t.join(timeout=90)
        assert not fatal and not any(t.is_alive() for t in threads),fatal
        finish=time.monotonic_ns()
        with base.connect() as c:after=base.require(c.request(op='benchmark_counters'))
        assert after['injected_stalls']==(1 if inject else 0)
        assert int(after['version'])-int(before['version'])==len(changes)
        if pin:
            for k in changes:assert base.require(pin.request(op='get',key=base.key(int(k)).hex()))['value']==base.value(int(k),initial.get(k,0)).hex()
        old=json.loads((base.DATA/'updated.json').read_text());old.update(changes);(base.DATA/'updated.json').write_text(json.dumps(old))
        ok=[r for r in records if r['status']=='ok'];lat=[r['ended']-r['planned'] for r in ok];assert len(records)==ops
        elapsed=(finish-t0)/1e9
        return dict(work=work,ops=ops,rate=rate,capacity_per_kind=capacity,batch_limit=batch_limit,scenario=scenario,elapsed_s=elapsed,start_ns=t0,finish_ns=finish,arm_begin_ns=arm_begin,arm_end_ns=arm_end,completed=len(ok),ops_s=len(ok)/elapsed,pin_reader=pin_reader,old_snapshot_checked_keys=len(changes) if pin else 0,p99_ms=base.quantile(lat,.99),max_ms=max(lat)/1e6 if lat else None,queue_rejected=sum(r['status']=='queue_full' for r in records),service_rejected=sum(r['status']=='resource_refused' for r in records),errors=[r for r in records if r['status']=='error'],by_kind={k:dict(completed=sum(r['kind']==k for r in ok),p99_ms=base.quantile([r['ended']-r['planned'] for r in ok if r['kind']==k],.99)) for k in set(kinds)},sync_attempts=after['sync_attempts']-before['sync_attempts'],stall_counters=after,batches=batches,records=records)
    finally:
        if pin:pin.close()

def main():
    import sys
    if len(sys.argv)<2 or sys.argv[1]!='measure':runpy.run_path(str(ROOT/'resource-envelope/client.py'),run_name='__main__');return
    p=argparse.ArgumentParser();p.add_argument('action');p.add_argument('--work');p.add_argument('--ops',type=int);p.add_argument('--rate',type=float);p.add_argument('--pin-reader',type=int);a=p.parse_args()
    result=measure(a.work,a.ops,a.rate,64,bool(a.pin_reader),batch_limit=int(os.environ['CATDB_BATCH_LIMIT']),scenario=os.environ['CATDB_STALL_SCENARIO'])
    (base.DATA/(a.work+'-raw.json')).write_text(json.dumps(result));result.pop('records');result.pop('batches');print(json.dumps(result))
if __name__=='__main__':main()
