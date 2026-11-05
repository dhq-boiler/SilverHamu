"""Finite, completion-independent arrivals with one FIFO per connection."""
import queue
import threading
import time

def planned_at(origin,index,rate):
    if rate<=0 or index<0:raise ValueError('positive rate and nonnegative index required')
    return origin+index*1_000_000_000//rate

def admit(target,event):
    try:target.put_nowait(event);return True
    except queue.Full:
        event['status']='rejected'
        event['finished_ns']=event['admitted_ns']
        return False

def execute(factory,operations,clients,rate,capacity=64):
    if not 1<=operations<=1_000_000 or clients not in (1,4) or operations%clients or rate<=0:
        raise ValueError('invalid workload')
    if capacity<clients or capacity%clients:raise ValueError('invalid queue capacity')
    queues=[queue.Queue(capacity//clients) for _ in range(clients)]
    ready=threading.Barrier(clients+1);done=threading.Barrier(clients)
    stop=threading.Event();fatal=[];events=[];probes=[[] for _ in range(clients)]
    counters={};high_water=[0]*clients
    def worker(w):
        adapter=None
        try:
            adapter=factory(w)
            adapter.warm()
            if w==0:counters['before']=adapter.counters()
            ready.wait(timeout=120)
            while not stop.is_set():
                try:event=queues[w].get(timeout=.1)
                except queue.Empty:continue
                if event is None:break
                event['sent_ns']=time.perf_counter_ns()
                try:
                    adapter.update(event)
                    event['status']='completed'
                except Exception as error:
                    event['status']='failed';event['error']=str(error)
                event['finished_ns']=time.perf_counter_ns()
                if event['status']=='completed':
                    probe=adapter.probe(event)
                    if probe is not None:probes[w].append(probe)
            if not stop.is_set():
                done.wait(timeout=180)
                if w==0:counters['after']=adapter.counters()
        except BaseException as error:
            fatal.append(repr(error));stop.set();ready.abort();done.abort()
        finally:
            if adapter is not None:adapter.close()
    threads=[threading.Thread(target=worker,args=(w,),daemon=True) for w in range(clients)]
    for thread in threads:thread.start()
    try:
        ready.wait(timeout=120)
        origin=time.perf_counter_ns()+100_000_000
        for i in range(operations):
            planned=planned_at(origin,i,rate)
            while not stop.is_set():
                remaining=planned-time.perf_counter_ns()
                if remaining<=0:break
                stop.wait(remaining/1e9)
            if stop.is_set():break
            w=i%clients
            event={'id':i,'worker':w,'sequence':i//clients,'planned_ns':planned,
                   'admitted_ns':time.perf_counter_ns(),'status':'queued'}
            events.append(event)
            admit(queues[w],event)
            high_water[w]=max(high_water[w],queues[w].qsize())
        for target in queues:
            while not stop.is_set():
                try:target.put(None,timeout=.1);break
                except queue.Full:continue
    except threading.BrokenBarrierError:
        stop.set()
    deadline=time.monotonic()+180
    for thread in threads:thread.join(max(0,deadline-time.monotonic()))
    if any(thread.is_alive() for thread in threads):
        stop.set();raise RuntimeError('workers did not drain within 180 seconds')
    if fatal:raise RuntimeError('worker failure: '+str(fatal))
    assert len(events)==operations and all(e['status'] in ('completed','failed','rejected') for e in events)
    return {'events':events,'probes':[p for group in probes for p in group],
            'counters':counters,'queue_capacity':capacity,'queue_high_water':high_water,
            'origin_ns':origin,'drained_ns':time.perf_counter_ns()}
