"""Same seeded operations; pull a bounded last-request trace after slow samples."""
import sys,json,time,threading
sys.path.insert(0,'/bench-src')
import runner as base

profile=False
records=[]
lock=threading.Lock()
dropped=0
THRESHOLD_NS=10_000_000

class Single(base.SingleRemote):
 def request(self,**kw):
  if not profile:return super().request(**kw)
  self.request_index=getattr(self,'request_index',0)+1
  started=time.perf_counter_ns()
  response=super().request(**kw)
  self.last_request_ns=time.perf_counter_ns()-started
  return response
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
  base.Remote=Single
  r=base.measure(engine,sys.argv[3],int(sys.argv[4]),int(sys.argv[5]))
 if profile:r['profile']={'records':records,'dropped':dropped,'limit':base.OPS,'threshold_ns':THRESHOLD_NS}
 print(json.dumps(r))
