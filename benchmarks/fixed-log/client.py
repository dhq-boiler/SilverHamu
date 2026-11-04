"""Use the frozen comparison operations; optional timing data contains no keys/values."""
import sys,json,time,pathlib
sys.path.insert(0,'/bench-src')
import runner as base
samples=[]
Original=base.Remote
class Profiled(Original):
 def request(self,**kw):
  start=time.perf_counter_ns();r=super().request(**kw);elapsed=time.perf_counter_ns()-start
  samples.append({'op':kw['op'],'client_ns':elapsed,'server':r.get('diagnostic')})
  return r
if __name__=='__main__':
 action=sys.argv[1];engine='catdb-service'
 if action=='init':base.initialize(engine);r={'initialized':base.ROWS}
 elif action=='verify':r=base.verify(engine)
 elif action=='metrics':
  c=base.Remote();r=c.request(op='fixed_log_metrics');c.close()
 else:
  workload=sys.argv[2];clients=int(sys.argv[3]);rep=int(sys.argv[4]);profile=sys.argv[5]=='1'
  if profile:base.Remote=Profiled
  r=base.measure(engine,workload,clients,rep)
  if profile:r['profile']=samples
 print(json.dumps(r))
