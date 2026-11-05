"""Same seeded operations; pull a bounded last-request trace after slow samples."""
import sys,json,time,threading
sys.path.insert(0,'/bench-src')
import runner as base
from worker import worker

profile=False
records=[]
lock=threading.Lock()
dropped=0
THRESHOLD_NS=10_000_000

class Single(base.Remote):
 def request(self,**kw):
  if not profile:return super().request(**kw)
  self.request_index=getattr(self,'request_index',0)+1
  started=time.perf_counter_ns()
  response=super().request(**kw)
  self.last_request_ns=time.perf_counter_ns()-started
  return response
 def begin(self):self.pending=None
 def get(self,k):
  r=self.request(op='get_latest',key=k.hex());return bytes.fromhex(r['value']) if r.get('value') is not None else None
 def scan(self,k,rows=100):return [(bytes.fromhex(a),bytes.fromhex(b)) for a,b in self.request(op='scan_latest',start=k.hex(),rows=rows,bytes=64000,visits=65536)['rows']]
 def put(self,k,v):self.pending=(k,v)
 def commit(self):
  k,v=self.pending;self.request(op='put_commit',key=k.hex(),value=v.hex());self.pending=None
 def rollback(self):self.pending=None
 def after_sample(self,elapsed,worker_id,sequence):
  global dropped
  if not profile:return
  previous=getattr(self,'maximum_ns',0)
  self.maximum_ns=max(elapsed,previous)
  if elapsed<THRESHOLD_NS and elapsed<=previous:return
  row={'operation_ns':elapsed,'request_ns':self.last_request_ns,'worker':worker_id,
       'sequence':sequence,'request_index':self.request_index,'reason':'threshold' if elapsed>=THRESHOLD_NS else 'record','ok':True}
  started=time.perf_counter_ns()
  try:
   response=self.client.request(op='diagnostic_last')
   row['diagnostic']=response.get('diagnostic')
   if not response.get('ok') or row['diagnostic'] is None:raise ValueError('diagnostic unavailable')
   if row['diagnostic']['request_index']!=self.request_index:raise ValueError('request correspondence mismatch')
  except Exception as error:row['probe_error']=str(error)
  row['probe_ns']=time.perf_counter_ns()-started
  with lock:
   if len(records)<base.OPS:records.append(row)
   else:dropped+=1

if __name__=='__main__':
 single=sys.argv[1]=='1';action=sys.argv[2];engine='catdb-service'
 if action=='init':base.initialize(engine);r={'initialized':base.ROWS}
 elif action=='verify':r=base.verify(engine)
 else:
  profile=len(sys.argv)>6 and sys.argv[6]=='1'
  assert single
  base.Remote=Single;base.worker=worker
  r=base.measure(engine,sys.argv[3],int(sys.argv[4]),int(sys.argv[5]))
 if profile:r['profile']={'records':records,'dropped':dropped,'limit':base.OPS,'threshold_ns':THRESHOLD_NS}
 print(json.dumps(r))
