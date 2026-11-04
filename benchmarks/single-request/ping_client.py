"""Auxiliary version-only healthcheck timing using the same Python worker harness."""
import sys,json
sys.path.insert(0,'/bench-src')
import runner as base
class Ping(base.Remote):
 def begin(self):pass
 def rollback(self):pass
 def get(self,k):
  r=self.request(op='ping');assert int(r['version'])>=0;return b'x'
if __name__=='__main__':
 action=sys.argv[2];engine='catdb-service'
 if action=='init':base.initialize(engine);r={'initialized':base.ROWS}
 elif action=='verify':r=base.verify(engine)
 else:
  base.Remote=Ping
  r=base.measure(engine,'get',int(sys.argv[4]),int(sys.argv[5]));r['workload']='ping'
 print(json.dumps(r))
