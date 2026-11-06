"""Same update schedule, with optional bounded libc I/O observations."""
import ctypes as c,json,os,pathlib,sys,time
sys.path.insert(0,'/bench')
os.environ.setdefault('BENCH_PASSWORD','synthetic-local-only')
os.environ.setdefault('BENCH_TOKEN','synthetic-local-only')
plan=json.loads(pathlib.Path(sys.argv[1]).read_text());os.environ['BENCH_ROWS']=str(plan['rows'])
import runner as base
engine=sys.argv[2];diag=sys.argv[3]=='1';pin=sys.argv[4]=='1'
native=engine=='catdb-embedded';ops=plan['operations']
assert 0<len(ops)<=8000 and all(0<=k<plan['rows'] and stamp>0 for k,stamp in ops)
base.initialize(engine);initial=base.verify(engine);a=base.adapter(engine)
if native:base.Native.check(a.lib.ndb_tx_set_group_commit(a.db,1))
a.begin();assert a.get(base.key(0))==base.value(0);a.rollback()
old=None
if pin:old=base.adapter(engine);old.begin();assert old.get(base.key(0))==base.value(0)
class Event(c.Structure):
    _fields_=[(k,c.c_int64) for k in ['req','kind','fd','offset','bytes','result','error']]+[(k,c.c_uint64) for k in ['start','end']]
lib=None;fds={}
for p in pathlib.Path('/proc/self/fd').iterdir():
    try:
        target=p.resolve(strict=True)
        if str(target).startswith('/data/') and target.is_file():
            fds[int(p.name)]={'path':target.name,'size':target.stat().st_size}
    except FileNotFoundError:pass
assert fds
if diag:
    lib=c.CDLL('/usr/local/lib/libiotrace.so');lib.trace_events.restype=c.POINTER(Event)
    lib.trace_request.argtypes=[c.c_int];lib.trace_track.argtypes=[c.c_int]
    for fd in fds:assert lib.trace_track(fd)==0
samples=[];bounds=[];start=time.monotonic_ns()
for i,(k,stamp) in enumerate(ops):
    if diag:lib.trace_request(i)
    t=time.monotonic_ns();a.begin();a.put(base.key(k),base.value(k,stamp));mid=time.monotonic_ns();a.commit();end=time.monotonic_ns()
    if diag:lib.trace_request(-1)
    samples.append(end-t);bounds.append([t,mid,end])
elapsed=(time.monotonic_ns()-start)/1e9
events=[]
if diag:
    assert lib.trace_overflow()==0 and lib.trace_closed()==0
    count=lib.trace_count();assert count<=200000
    ptr=lib.trace_events()
    events=[[getattr(ptr[i],k) for k,_ in Event._fields_] for i in range(count)]
final_sizes={p.name:p.stat().st_size for p in pathlib.Path('/data').iterdir() if p.is_file()}
changed={str(k):stamp for k,stamp in ops}
if not pin:
    old=base.adapter(engine);old.begin();assert old.get(base.key(0))==base.value(0,changed.get('0',0))
a.begin();a.put(base.key(0),base.value(0,2**64-1));a.commit()
for k in range(plan['rows']):assert old.get(base.key(k))==base.value(k,0 if pin else changed.get(str(k),0))
old.rollback();old.close();a.close();changed['0']=2**64-1
pathlib.Path('/data/updated.json').write_text(json.dumps(changed))
latest=base.verify(engine);reopened=base.verify(engine);assert latest['sha256']==reopened['sha256']
print(json.dumps({'engine':engine,'diagnostic':diag,'pinned':pin,'rows':plan['rows'],'samples_ns':samples,'bounds':bounds,'elapsed_s':elapsed,'fds':fds,'final_sizes':final_sizes,'events':events,'checks':['initial','old','latest','reopen'],'hashes':[initial['sha256'],latest['sha256'],reopened['sha256']]}))
