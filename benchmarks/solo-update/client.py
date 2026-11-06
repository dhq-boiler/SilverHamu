"""Same precomputed key/stamp schedule for C ABI, SQLite and Rust."""
import json,os,pathlib,sys,time
sys.path.insert(0,'/bench')
os.environ.setdefault('BENCH_PASSWORD','synthetic-local-only')
os.environ.setdefault('BENCH_TOKEN','synthetic-local-only')
plan=json.loads(pathlib.Path(sys.argv[1]).read_text());os.environ['BENCH_ROWS']=str(plan['rows'])
import runner as base
engine=sys.argv[2];group=sys.argv[3]=='on';native=engine=='catdb-embedded'
ops=plan['operations'];assert 0<len(ops)<=8000
assert all(0<=k<plan['rows'] and stamp>0 for k,stamp in ops)
base.initialize(engine);initial=base.verify(engine)
a=base.adapter(engine)
if native:base.Native.check(a.lib.ndb_tx_set_group_commit(a.db,int(group)))
pin=len(sys.argv)>4 and sys.argv[4]=='1'
old=None
if pin:old=base.adapter(engine);old.begin();assert old.get(base.key(0))==base.value(0)
samples=[];start=time.perf_counter_ns()
for k,stamp in ops:
    t=time.perf_counter_ns();a.begin();a.put(base.key(k),base.value(k,stamp));a.commit();samples.append(time.perf_counter_ns()-t)
elapsed=(time.perf_counter_ns()-start)/1e9
changed={str(k):stamp for k,stamp in ops}
if not pin:
    old=base.adapter(engine);old.begin();assert old.get(base.key(0))==base.value(0,changed.get('0',0))
a.begin();a.put(base.key(0),base.value(0,2**64-1));a.commit()
for k in range(plan['rows']):assert old.get(base.key(k))==base.value(k,0 if pin else changed.get(str(k),0))
old.rollback();old.close();a.close();changed['0']=2**64-1
pathlib.Path('/data/updated.json').write_text(json.dumps(changed))
latest=base.verify(engine)
reopened=base.verify(engine)
assert latest['sha256']==reopened['sha256']
print(json.dumps({'api':'cabi' if native else 'sqlite','group':group if native else None,'diagnostic':False,'pinned_during_timing':pin,'rows':plan['rows'],'samples_ns':samples,'elapsed_s':elapsed,'sync_attempts':None,'checks':['initial','old','latest','reopen'],'hashes':[initial['sha256'],latest['sha256'],reopened['sha256']],'diagnostics':[]}))
