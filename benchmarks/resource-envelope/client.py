"""Fixed-arrival, bounded-queue workloads against disposable catdb fixtures."""
import argparse, hashlib, json, math, os, pathlib, queue, random, threading, time
from catdb_client import Client
from runner import key, value, ROWS, TOKEN

DATA = pathlib.Path('/data')

def connect(): return Client('database',7400,TOKEN,timeout=30)

def require(r):
    if not r.get('ok'): raise RuntimeError(str(r))
    return r

def verify():
    changes = json.loads((DATA/'updated.json').read_text())
    digest = hashlib.sha256()
    with connect() as c:
        require(c.request(op='begin_read'))
        for start in range(0,ROWS,100):
            rows = require(c.request(op='scan',start=key(start).hex(),rows=100,bytes=64000,visits=65536))['rows']
            expected = [(key(i).hex(),value(i,changes.get(str(i),0)).hex()) for i in range(start,min(start+100,ROWS))]
            assert [tuple(x) for x in rows] == expected, ('mismatch',start)
            for k,v in rows: digest.update(bytes.fromhex(k)+bytes.fromhex(v))
        assert require(c.request(op='get',key=key(ROWS).hex()))['value'] is None
        assert not require(c.request(op='scan',start=key(ROWS).hex(),rows=100))['rows']
        require(c.request(op='rollback'))
    return {'rows':ROWS,'sha256':digest.hexdigest(),'updated':len(changes),'passed':True}

def quantile(samples,p):
    a=sorted(samples)
    return a[max(0,math.ceil(p*len(a))-1)]/1e6 if a else None

def measure(work,ops,rate,capacity,pin_reader=True):
    assert ops>0 and rate>0 and capacity>0 and (ops+4)//5<=ROWS
    initial=json.loads((DATA/'updated.json').read_text())
    pin=connect() if pin_reader else None
    if pin is not None: require(pin.request(op='begin_read'))
    # Separate queues prevent writes from consuming every reader slot.
    qs={'read':queue.Queue(capacity),'write':queue.Queue(capacity)}
    records=[]; changes={}; lock=threading.Lock(); ready=threading.Barrier(6 if work=='mixed' else 5)
    start_event=threading.Event(); setup_errors=[]
    def worker(kind):
        try: c=connect()
        except Exception as e:
            setup_errors.append(str(e)); ready.abort(); return
        try:
            ready.wait(timeout=30); start_event.wait(timeout=30)
            while True:
                item=qs[kind].get()
                if item is None: qs[kind].task_done(); break
                seq,k,planned,enqueued=item; began=time.monotonic_ns()
                rec={'seq':seq,'kind':kind,'planned':planned,'enqueued':enqueued,'began':began,'status':'ok'}
                try:
                    if kind=='write':
                        response=require(c.request(op='put_commit',key=key(k).hex(),value=value(k,seq+1).hex()))
                        if 'diagnostic' in response: rec['diagnostic']=response['diagnostic']
                        with lock: changes[str(k)]=seq+1
                    else:
                        b=bytes.fromhex(require(c.request(op='get_latest',key=key(k).hex()))['value'])
                        assert len(b)==256 and b[:8]==key(k) and b[16:]==b'x'*240
                        stamp=int.from_bytes(b[8:16],'big')
                        assert stamp==initial.get(str(k),0) or (work=='mixed' and (stamp-1)%5==0 and (stamp-1)//5==k)
                except Exception as e: rec.update(status='error',error=str(e))
                rec['ended']=time.monotonic_ns()
                with lock: records.append(rec)
                qs[kind].task_done()
        finally: c.close()
    kinds=['read']*4 if work=='read' else ['read']+['write']*4
    threads=[threading.Thread(target=worker,args=(k,),daemon=True) for k in kinds]
    for t in threads:t.start()
    ready.wait(timeout=35)
    if setup_errors: raise RuntimeError(setup_errors)
    t0=time.monotonic_ns();start_event.set();rng=random.Random(739)
    for seq in range(ops):
        planned=t0+int(seq*1e9/rate)
        while True:
            wait=(planned-time.monotonic_ns())/1e9
            if wait<=0:break
            time.sleep(wait)
        kind='write' if work=='mixed' and seq%5==0 else 'read'
        k=seq//5 if kind=='write' else rng.randrange(ROWS)
        enqueued=time.monotonic_ns()
        try:qs[kind].put_nowait((seq,k,planned,enqueued))
        except queue.Full:records.append({'seq':seq,'kind':kind,'planned':planned,'enqueued':enqueued,'ended':time.monotonic_ns(),'status':'queue_full'})
    for kind in kinds:qs[kind].put(None,timeout=60)
    for t in threads:t.join(timeout=90)
    assert not any(t.is_alive() for t in threads), 'workers stranded'
    elapsed=(time.monotonic_ns()-t0)/1e9
    if pin is not None:
        for k in changes:
            assert require(pin.request(op='get',key=key(int(k)).hex()))['value']==value(int(k),initial.get(k,0)).hex()
        require(pin.request(op='rollback'));pin.close()
    old=json.loads((DATA/'updated.json').read_text());old.update(changes);(DATA/'updated.json').write_text(json.dumps(old))
    ok=[r for r in records if r['status']=='ok']; lat=[r['ended']-r['planned'] for r in ok]
    assert len(records)==ops
    return {'work':work,'ops':ops,'rate':rate,'capacity_per_kind':capacity,'elapsed_s':elapsed,'start_ns':t0,'finish_ns':t0+int(elapsed*1e9),
            'completed':len(ok),'ops_s':len(ok)/elapsed,'pin_reader':pin_reader,'old_snapshot_checked_keys':len(changes) if pin_reader else 0,'p99_ms':quantile(lat,.99),'max_ms':max(lat)/1e6 if lat else None,
            'queue_rejected':sum(r['status']=='queue_full' for r in records),'errors':[r for r in records if r['status']=='error'],
            'by_kind':{k:{'completed':sum(r['kind']==k for r in ok),'p99_ms':quantile([r['ended']-r['planned'] for r in ok if r['kind']==k],.99)} for k in set(kinds)},
            'records':records}

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('action',choices=['ready','verify','measure']);p.add_argument('--work',default='read');p.add_argument('--ops',type=int,default=8000);p.add_argument('--rate',type=float,default=1000);p.add_argument('--capacity',type=int,default=64);p.add_argument('--pin-reader',type=int,choices=[0,1],default=1);a=p.parse_args()
    if a.action=='ready':
        start=time.monotonic_ns();last=None
        for _ in range(600):
            try:
                with connect() as c:require(c.request(op='ping'))
                print(json.dumps({'ready_wait_ms':(time.monotonic_ns()-start)/1e6}));break
            except (OSError,RuntimeError) as e:
                last=str(e)
                limits=sorted(DATA.glob('*-limits.json'),key=lambda p:p.stat().st_mtime_ns)
                if limits:
                    phase=limits[-1].name.removesuffix('-limits.json')
                    ended=DATA/f'{phase}-exit.json'
                    if ended.exists():raise RuntimeError('server exited before ready: '+ended.read_text()+' '+(DATA/f'{phase}-server.log').read_text())
                time.sleep(.1)
        else:raise RuntimeError('not ready: '+str(last))
    else:
        result=verify() if a.action=='verify' else measure(a.work,a.ops,a.rate,a.capacity,bool(a.pin_reader))
        if a.action=='measure':
            (DATA/f'{a.work}-raw.json').write_text(json.dumps(result))
            result.pop('records')
        print(json.dumps(result))
