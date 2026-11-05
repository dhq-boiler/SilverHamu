"""Same seeded operations and Python scheduler; change only the API adapter."""
import sys,json,time,threading
records=[]
record_lock=threading.Lock()
dropped=0
profile=False
sys.path.insert(0,'/bench-src')
import runner as base
class Single(base.Remote):
 def request(self,**kw):
  global dropped
  if not profile or kw.get('op') not in ('put_commit','delete_commit','commit'):
   return super().request(**kw)
  started=time.perf_counter_ns()
  response=self.client.request(**kw)
  elapsed=time.perf_counter_ns()-started
  record={'request_ns':elapsed,'worker':threading.get_ident(),'ok':response.get('ok'),
          'error':response.get('error'),'diagnostic':response.get('diagnostic')}
  with record_lock:
   if len(records)<base.OPS: records.append(record)
   else:dropped+=1
  if not response.get('ok'):raise RuntimeError(str(response))
  return response

 def begin(self): self.pending=None
 def get(self,k):
  r=self.request(op='get_latest',key=k.hex());return bytes.fromhex(r['value']) if r.get('value') is not None else None
 def scan(self,k,rows=100):return [(bytes.fromhex(a),bytes.fromhex(b)) for a,b in self.request(op='scan_latest',start=k.hex(),rows=rows,bytes=64000,visits=65536)['rows']]
 def put(self,k,v):self.pending=(k,v)
 def commit(self):
  k,v=self.pending;self.request(op='put_commit',key=k.hex(),value=v.hex());self.pending=None
 def rollback(self):self.pending=None
if __name__=='__main__':
 single=sys.argv[1]=='1';action=sys.argv[2];engine='catdb-service'
 if action=='init':base.initialize(engine);r={'initialized':base.ROWS}
 elif action=='verify':r=base.verify(engine)
 else:
  profile=len(sys.argv)>6 and sys.argv[6]=='1'
  if single:base.Remote=Single
  r=base.measure(engine,sys.argv[3],int(sys.argv[4]),int(sys.argv[5]))
 if profile:r['profile']={'records':records,'dropped':dropped,'limit':base.OPS}
 print(json.dumps(r))
