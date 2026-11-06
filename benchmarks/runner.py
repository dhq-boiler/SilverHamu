"""Synthetic API benchmark; never point this runner at an existing database."""
import argparse, concurrent.futures, ctypes as c, hashlib, json, math, os
import pathlib, random, sqlite3, struct, threading, time
from catdb_client import Client

ROWS = int(os.environ.get('BENCH_ROWS', '100000'))
OPS = int(os.environ.get('BENCH_OPS', '8000'))
PASSWORD = os.environ['BENCH_PASSWORD']
TOKEN = os.environ['BENCH_TOKEN']

def key(i): return i.to_bytes(8, 'big')
def value(i, stamp=0): return key(i) + int(stamp).to_bytes(8, 'big') + b'x' * 240

class Native:
    lib = None
    db = None
    @classmethod
    def connect(cls, create=False):
        if cls.lib is None:
            cls.lib = c.CDLL('/usr/local/lib/libnewdbms.so')
            p,u=c.c_void_p,c.c_uint64
            signatures={'db_create':[p,u,p,p], 'db_open':[p,u,p], 'db_close':[u],
                'begin':[u,p,p], 'get':[u,p,u,p,u,p,p], 'put':[u,p,u,p,u],
                'commit':[u,p], 'rollback':[u], 'set_group_commit':[u,c.c_uint32],
                'checkpoint':[u,p,p,p,p],
                'scan_page':[u,p,u,p,u,c.c_uint32,p,u,c.c_uint32,u,u,u,p,u,p]}
            for n,a in signatures.items(): getattr(cls.lib,'ndb_tx_'+n).argtypes=a
        if cls.db is None:
            out=c.c_uint64(); path=b'/data/catdb.wal'
            args=(path,len(path),b'catdb-benchmark1',c.byref(out)) if create else (path,len(path),c.byref(out))
            cls.check(getattr(cls.lib,'ndb_tx_db_'+('create' if create else 'open'))(*args))
            cls.db=out.value
            cls.check(cls.lib.ndb_tx_set_group_commit(cls.db,1))
    @staticmethod
    def check(status):
        if status: raise RuntimeError('catdb ABI status '+str(status))
    def __init__(self):
        self.connect(); self.tx=None; self.output=c.create_string_buffer(65536)
    def begin(self):
        tx,v=c.c_uint64(),c.c_uint64()
        self.check(self.lib.ndb_tx_begin(self.db,c.byref(tx),c.byref(v))); self.tx=tx.value
    def get(self,k):
        n,f=c.c_uint64(),c.c_uint64()
        self.check(self.lib.ndb_tx_get(self.tx,k,len(k),self.output,65536,c.byref(n),c.byref(f)))
        return c.string_at(self.output, n.value) if f.value else None
    def scan(self,k,rows=100):
        n=c.c_uint64()
        self.check(self.lib.ndb_tx_scan_page(self.tx,k,len(k),None,0,0,None,0,0,rows,64000,65536,self.output,65536,c.byref(n)))
        data=c.string_at(self.output, n.value); count,visits,payload,has_after,after_len=struct.unpack_from('<5Q',data)
        pos=40+after_len; result=[]
        for _ in range(count):
            a,b=struct.unpack_from('<II',data,pos);pos+=8
            result.append((data[pos:pos+a],data[pos+a:pos+a+b]));pos+=a+b
        return result
    def put(self,k,v): self.check(self.lib.ndb_tx_put(self.tx,k,len(k),v,len(v)))
    def commit(self):
        tx=self.tx;self.tx=None;v=c.c_uint64();self.check(self.lib.ndb_tx_commit(tx,c.byref(v)))
    def rollback(self):
        if self.tx is not None: self.check(self.lib.ndb_tx_rollback(self.tx));self.tx=None
    def close(self): self.rollback()
    @classmethod
    def shutdown(cls):
        if cls.db is not None: cls.check(cls.lib.ndb_tx_db_close(cls.db));cls.db=None

class Remote:
    def __init__(self): self.client=Client('database',7400,TOKEN,timeout=120)
    def request(self,**kw):
        r=self.client.request(**kw)
        if not r.get('ok'): raise RuntimeError(str(r))
        return r
    def begin(self):self.request(op='begin')
    def get(self,k):
        r=self.request(op='get',key=k.hex());return bytes.fromhex(r['value']) if r.get('value') is not None else None
    def scan(self,k,rows=100):return [(bytes.fromhex(a),bytes.fromhex(b)) for a,b in self.request(op='scan',start=k.hex(),rows=rows,bytes=64000,visits=65536)['rows']]
    def put(self,k,v):self.request(op='put',key=k.hex(),value=v.hex())
    def commit(self):self.request(op='commit')
    def rollback(self):self.request(op='rollback')
    def close(self):self.client.close()

class SingleRemote(Remote):
    def begin(self): self.pending=None
    def get(self,k):
        r=self.request(op='get_latest',key=k.hex())
        return bytes.fromhex(r['value']) if r.get('value') is not None else None
    def scan(self,k,rows=100):
        return [(bytes.fromhex(a),bytes.fromhex(b)) for a,b in self.request(op='scan_latest',start=k.hex(),rows=rows,bytes=64000,visits=65536)['rows']]
    def put(self,k,v): self.pending=(k,v)
    def commit(self):
        k,v=self.pending
        self.request(op='put_commit',key=k.hex(),value=v.hex())
        self.pending=None
    def rollback(self): self.pending=None

class SQL:
    def __init__(self,engine):
        self.engine=engine
        if engine=='sqlite':
            self.db=sqlite3.connect('/data/sqlite.db',isolation_level=None,timeout=120)
            self.db.execute('PRAGMA journal_mode=WAL');self.db.execute('PRAGMA synchronous=FULL')
            self.db.execute('PRAGMA cache_size=-65536');self.db.execute('PRAGMA busy_timeout=120000')
            self.mark='?'
        elif engine=='postgres':
            import psycopg
            self.db=psycopg.connect(host='database',dbname='bench',user='bench',password=PASSWORD)
            self.db.isolation_level=psycopg.IsolationLevel.SERIALIZABLE;self.mark='%s'
        elif engine=='mysql':
            import pymysql
            self.db=pymysql.connect(host='database',database='bench',user='bench',password=PASSWORD,autocommit=False)
            with self.db.cursor() as cur:cur.execute('SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE')
            self.mark='%s'
        elif engine=='oracle':
            import oracledb
            self.db=oracledb.connect(user='bench',password=PASSWORD,dsn='database:1521/FREEPDB1')
            self.mark=':1'
            with self.db.cursor() as cur:
                cur.execute("ALTER SESSION SET COMMIT_WAIT='WAIT'")
                cur.execute("ALTER SESSION SET COMMIT_LOGGING='IMMEDIATE'")
        self.cur=self.db.cursor()
    def begin(self):
        if self.engine=='sqlite':self.cur.execute('BEGIN')
        elif self.engine=='oracle':self.cur.execute('SET TRANSACTION ISOLATION LEVEL SERIALIZABLE')
    def get(self,k):
        self.cur.execute('SELECT v FROM kv WHERE k='+self.mark,(k,));r=self.cur.fetchone();return bytes(r[0]) if r else None
    def scan(self,k,rows=100):
        tail=f' FETCH FIRST {int(rows)} ROWS ONLY' if self.engine=='oracle' else f' LIMIT {int(rows)}'
        self.cur.execute('SELECT k,v FROM kv WHERE k >= '+self.mark+' ORDER BY k'+tail,(k,))
        return [(bytes(a),bytes(b)) for a,b in self.cur.fetchall()]
    def put(self,k,v):
        marks=(':1',':2') if self.engine=='oracle' else (self.mark,self.mark)
        self.cur.execute(f'UPDATE kv SET v={marks[0]} WHERE k={marks[1]}',(v,k))
        assert self.cur.rowcount==1
    def commit(self):self.db.commit()
    def rollback(self):self.db.rollback()
    def close(self):self.cur.close();self.db.close()

def adapter(engine):return Native() if engine=='catdb-embedded' else Remote() if engine=='catdb-service' else SQL(engine)
def initialize(engine):
    if engine.startswith('catdb'):
        Native.connect(create=True);a=Native();a.begin();assert a.get(key(0)) is None and a.scan(key(0))==[];a.rollback()
        for start in range(0,ROWS,1000):
            a.begin()
            for i in range(start,min(ROWS,start+1000)):a.put(key(i),value(i))
            a.commit()
        outputs=[c.c_uint64() for _ in range(4)]
        Native.check(Native.lib.ndb_tx_checkpoint(Native.db,*[c.byref(x) for x in outputs]))
        a.close();Native.shutdown()
        for p in pathlib.Path('/data').iterdir():os.chown(p,10001,10001)
    else:
        a=SQL(engine)
        types={'sqlite':'k BLOB PRIMARY KEY, v BLOB NOT NULL','postgres':'k BYTEA PRIMARY KEY, v BYTEA NOT NULL','mysql':'k BINARY(8) PRIMARY KEY, v VARBINARY(256) NOT NULL','oracle':'k RAW(8) PRIMARY KEY, v RAW(256) NOT NULL'}
        suffix=' WITHOUT ROWID' if engine=='sqlite' else ' ENGINE=InnoDB' if engine=='mysql' else ''
        a.cur.execute('CREATE TABLE kv ('+types[engine]+')'+suffix);a.commit()
        a.begin();assert a.get(key(0)) is None and a.scan(key(0))==[];a.rollback()
        marks=':1,:2' if engine=='oracle' else a.mark+','+a.mark
        for start in range(0,ROWS,1000):
            if engine=='oracle':a.cur.execute('SET TRANSACTION ISOLATION LEVEL READ COMMITTED')
            else:a.begin()
            a.cur.executemany('INSERT INTO kv(k,v) VALUES('+marks+')',[(key(i),value(i)) for i in range(start,min(ROWS,start+1000))]);a.commit()
        a.close()
    pathlib.Path('/data/updated.json').write_text('{}')

def verify(engine):
    changed=json.loads(pathlib.Path('/data/updated.json').read_text());a=adapter(engine)
    digest=hashlib.sha256();expected=hashlib.sha256();a.begin()
    assert a.get(key(ROWS)) is None
    assert a.scan(key(ROWS))==[]
    assert len(a.scan(key(ROWS-1)))==1
    for start in range(0,ROWS,100):
        rows=a.scan(key(start))
        wanted=[(key(i),value(i,changed.get(str(i),0))) for i in range(start,min(start+100,ROWS))]
        assert rows==wanted, ('content mismatch',engine,start)
        for k,v in rows:digest.update(k+v)
        for k,v in wanted:expected.update(k+v)
    a.rollback();a.close();Native.shutdown()
    return {'rows':ROWS,'sha256':digest.hexdigest(),'expected_sha256':expected.hexdigest(),'updated_keys':len(changed),'empty_and_last':True}

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
            samples.append(time.perf_counter_ns()-began)
        except Exception as e:
            errors.append({'elapsed_ns':time.perf_counter_ns()-began,'error':str(e)})
            try:a.rollback()
            except Exception:pass
    a.close();return samples,errors,changed

def measure(engine,workload,clients,rep):
    if engine=='catdb-embedded':Native.connect()
    # Threads are intentional: both embedded adapters share a process and interpreter.
    # Service results are also client-observed Python-thread measurements, not engine ceilings.
    ready=threading.Barrier(clients+1);start=threading.Event()
    with concurrent.futures.ThreadPoolExecutor(max_workers=clients) as pool:
        futures=[pool.submit(worker,engine,workload,i,clients,rep,start,ready) for i in range(clients)]
        ready.wait(timeout=120);t0=time.perf_counter_ns();start.set()
        results=[f.result(timeout=600) for f in futures];elapsed=time.perf_counter_ns()-t0
    all_samples=[s for samples,_,_ in results for s in samples];errors=[e for _,es,_ in results for e in es]
    changes=json.loads(pathlib.Path('/data/updated.json').read_text())
    for _,_,cs in results:changes.update(cs)
    pathlib.Path('/data/updated.json').write_text(json.dumps(changes))
    Native.shutdown(); ordered=sorted(all_samples)
    def percentile(p):return ordered[max(0,math.ceil(len(ordered)*p)-1)]/1e6 if ordered else None
    return {'engine':engine,'workload':workload,'clients':clients,'rep':rep,'rows':ROWS,'value_bytes':256,'attempted':OPS,'completed':len(ordered),'errors':errors,'retries':0,'elapsed_s':elapsed/1e9,'ops_s':len(ordered)/(elapsed/1e9),'p50_ms':percentile(.5),'p95_ms':percentile(.95),'p99_ms':percentile(.99),'max_ms':max(ordered)/1e6 if ordered else None,'latencies_ns':all_samples}

def metadata(engine):
    if engine.startswith('catdb'):return {'version':'0.1.0','group_commit':True,'sync_before_ack':True}
    a=SQL(engine)
    queries={'sqlite':["select sqlite_version()","pragma journal_mode","pragma synchronous","pragma cache_size"],
    'postgres':["select version()","show fsync","show synchronous_commit","show shared_buffers","show transaction_isolation"],
    'mysql':["select version()","select @@innodb_flush_log_at_trx_commit,@@sync_binlog,@@log_bin,@@transaction_isolation,@@innodb_buffer_pool_size"],
    'oracle':["select banner_full from v$version","select sys_context('USERENV','DB_NAME') from dual"]}
    out={}
    for q in queries[engine]:a.cur.execute(q);out[q]=a.cur.fetchall()
    a.rollback();a.close();return out

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('action',choices=['init','verify','measure','metadata']);p.add_argument('engine');p.add_argument('--workload');p.add_argument('--clients',type=int);p.add_argument('--rep',type=int);args=p.parse_args()
    if args.action=='init':initialize(args.engine);result={'initialized':ROWS}
    elif args.action=='verify':result=verify(args.engine)
    elif args.action=='metadata':result=metadata(args.engine)
    else:
        if args.engine=='catdb-service' and os.environ.get('BENCH_SINGLE_REQUEST')=='1':Remote=SingleRemote
        result=measure(args.engine,args.workload,args.clients,args.rep)
    print(json.dumps(result,default=str))
