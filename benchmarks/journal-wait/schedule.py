"""Fixed deadlines and a fixed outstanding-request budget for both policies."""
import queue
import threading
import time

def execute(factory, operations, clients, rate, batch_size=1, capacity=64):
    if not 1 <= operations <= 1_000_000 or clients not in (1,4) or operations % clients or rate <= 0:
        raise ValueError('invalid workload')
    if capacity < clients or capacity % clients or not 1 <= batch_size <= 8:
        raise ValueError('invalid capacity or batch size')
    queues=[queue.Queue(capacity//clients) for _ in range(clients)]
    credits=[threading.BoundedSemaphore(capacity//clients) for _ in range(clients)]
    ready=threading.Barrier(clients+1); drained=threading.Barrier(clients)
    stop=threading.Event(); fatal=[]; events=[]; counters={}; batches=[[] for _ in range(clients)]
    timing={}; diagnostics=[None for _ in range(clients)]
    def worker(w):
        adapter=None
        try:
            adapter=factory(w);adapter.warm()
            ready.wait(timeout=120)
            if w==0:
                counters['before']=adapter.counters()
                timing['armed_client_ns']=time.perf_counter_ns()
                adapter.arm()
                timing['armed_done_ns']=time.perf_counter_ns()
            ready.wait(timeout=120)
            carry=None; ended=False
            while not stop.is_set():
                if carry is not None:
                    first=carry;carry=None
                else:
                    try:first=queues[w].get(timeout=.1)
                    except queue.Empty:continue
                if first is None:break
                group=[first]; keys={adapter.identity(first)}
                while len(group)<batch_size:
                    try:nxt=queues[w].get_nowait()
                    except queue.Empty:break
                    if nxt is None:ended=True;break
                    key=adapter.identity(nxt)
                    if key in keys:
                        carry=nxt;break
                    keys.add(key);group.append(nxt)
                sent=time.perf_counter_ns()
                for e in group:e['sent_ns']=sent
                try:
                    outcomes=adapter.update_many(group)
                    if len(outcomes)!=len(group):raise ValueError('missing per-update outcome')
                except Exception as error:
                    outcomes=[{'ok':False,'error':str(error)} for _ in group]
                finished=time.perf_counter_ns()
                batch_id=len(batches[w])
                batches[w].append({'id':batch_id,'worker':w,'size':len(group),'sent_ns':sent,'finished_ns':finished})
                for e,outcome in zip(group,outcomes):
                    e.update(finished_ns=finished,batch_id=batch_id,status='completed' if outcome.get('ok') else 'failed')
                    if not outcome.get('ok'):e['error']=outcome.get('error','missing outcome')
                    credits[w].release()
                if ended:break
            if not stop.is_set():
                drained.wait(timeout=180)
                if w==0:counters['after']=adapter.counters()
                drained.wait(timeout=180)
                diagnostics[w]=adapter.diagnostics()
        except BaseException as error:
            fatal.append(repr(error));stop.set();ready.abort();drained.abort()
        finally:
            if adapter is not None:
                try:adapter.close()
                except Exception as error:fatal.append(repr(error))
    threads=[threading.Thread(target=worker,args=(w,),daemon=True) for w in range(clients)]
    for t in threads:t.start()
    try:
        ready.wait(timeout=120);ready.wait(timeout=120)
        origin=time.perf_counter_ns()+100_000_000
        for i in range(operations):
            planned=origin+i*1_000_000_000//rate
            while not stop.is_set():
                remaining=planned-time.perf_counter_ns()
                if remaining<=0:break
                stop.wait(remaining/1e9)
            if stop.is_set():break
            w=i%clients
            e=dict(id=i,worker=w,sequence=i//clients,planned_ns=planned,admitted_ns=time.perf_counter_ns(),status='queued')
            events.append(e)
            if credits[w].acquire(blocking=False):
                queues[w].put_nowait(e)
            else:
                e.update(status='rejected',finished_ns=e['admitted_ns'])
        for q in queues:
            while not stop.is_set():
                try:q.put(None,timeout=.1);break
                except queue.Full:continue
    except BaseException:
        stop.set();ready.abort();drained.abort();raise
    finally:
        deadline=time.monotonic()+180
        for t in threads:t.join(max(0,deadline-time.monotonic()))
    if any(t.is_alive() for t in threads):raise RuntimeError('workers did not drain')
    if fatal:raise RuntimeError('worker failure: '+str(fatal))
    assert len(events)==operations and all(e['status'] in ('completed','failed','rejected') for e in events)
    return dict(events=events,counters=counters,batches=[b for group in batches for b in group],
                diagnostics=diagnostics,origin_ns=origin,drained_ns=time.perf_counter_ns(),outstanding_capacity=capacity,batch_limit=batch_size,**timing)
