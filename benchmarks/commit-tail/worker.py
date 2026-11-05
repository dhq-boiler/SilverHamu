from runner import adapter,random,ROWS,key,OPS,time,value

def worker(engine,workload,worker_id,clients,rep,start,ready):
    a=adapter(engine);rng=random.Random(424242+rep*100+worker_id)
    population=ROWS-99 if workload=='range100' else ROWS
    keys=[key(rng.randrange((population-worker_id+clients-1)//clients)*clients+worker_id) for _ in range(OPS//clients)]
    # Warm connections and query plans outside the timed interval.
    for k in keys[:50]:a.begin();a.get(k);a.rollback()
    ready.wait();start.wait();samples=[];errors=[];changed={}
    for sequence,k in enumerate(keys):
        began=time.perf_counter_ns()
        try:
            a.begin()
            if workload=='get':assert a.get(k) is not None
            elif workload=='range100':
                rows=a.scan(k);assert len(rows)==min(100,ROWS-int.from_bytes(k,'big'))
            else:
                stamp=rep*1000000000+clients*10000000+worker_id*OPS+sequence+1
                a.put(k,value(int.from_bytes(k,'big'),stamp))
            if workload=='update':a.commit();changed[str(int.from_bytes(k,'big'))]=stamp
            else:a.rollback()
            elapsed=time.perf_counter_ns()-began
            samples.append(elapsed)
            a.after_sample(elapsed,worker_id,sequence)
        except Exception as e:
            errors.append({'elapsed_ns':time.perf_counter_ns()-began,'error':str(e)})
            try:a.rollback()
            except Exception:pass
    a.close();return samples,errors,changed
