"""Same seeded operations and Python scheduler; change only the API adapter."""
import sys,json
sys.path.insert(0,'/bench-src')
import runner as base
class Single(base.Remote):
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
  if single:base.Remote=Single
  r=base.measure(engine,sys.argv[3],int(sys.argv[4]),int(sys.argv[5]))
 print(json.dumps(r))
