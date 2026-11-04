import os
import sys
import ctypes as c
import pathlib, tempfile, unittest, struct
ROOT=pathlib.Path(__file__).resolve().parents[1]
N=c.CDLL(os.environ.get("NEWDBMS_TEST_DLL", str(ROOT / "target/release" / ("newdbms.dll" if os.name == "nt" else "libnewdbms.dylib" if sys.platform == "darwin" else "libnewdbms.so"))))
U8=c.POINTER(c.c_uint8); U64=c.c_uint64; OUT=c.POINTER(U64)
for name,args in {
 'db_create':[U8,U64,U8,OUT], 'db_open':[U8,U64,OUT], 'db_close':[U64],
 'begin':[U64,OUT,OUT], 'get':[U64,U8,U64,U8,U64,OUT,OUT],
 'put':[U64,U8,U64,U8,U64], 'delete':[U64,U8,U64], 'commit':[U64,OUT], 'rollback':[U64],
 'scan':[U64,U8,U64,U8,U64,c.c_uint32,U8,U64,OUT,OUT],
 'checkpoint':[U64,OUT,OUT,OUT,OUT], 'db_info':[U64,U8,OUT]
}.items():getattr(N,'ndb_tx_'+name).argtypes=args

def buf(value):return (c.c_uint8*max(1,len(value)))(*value)
class Checks(unittest.TestCase):
 def setUp(self):
  artifacts=(ROOT/'artifacts').resolve();artifacts.mkdir(exist_ok=True)
  self.temp=tempfile.TemporaryDirectory(prefix='abi-tx-',dir=artifacts);self.root=pathlib.Path(self.temp.name).resolve();assert self.root.is_relative_to(artifacts)
  self.path=str(self.root/'native.wal').encode();self.identity=bytes(range(16));out=U64()
  self.assertEqual(N.ndb_tx_db_create(buf(self.path),len(self.path),buf(self.identity),c.byref(out)),0);self.db=out.value;self.txs=[]
 def tearDown(self):
  for tx in self.txs:N.ndb_tx_rollback(tx)
  N.ndb_tx_db_close(self.db);self.temp.cleanup()
 def begin(self):
  tx,version=U64(),U64();self.assertEqual(N.ndb_tx_begin(self.db,c.byref(tx),c.byref(version)),0);self.txs.append(tx.value);return tx.value
 def test_bv_null_commit_output_does_not_consume_transaction(self):
  tx=self.begin();self.assertEqual(N.ndb_tx_put(tx,None,0,None,0),0)
  self.assertEqual(N.ndb_tx_commit(tx,None),1);version=U64();self.assertEqual(N.ndb_tx_commit(tx,c.byref(version)),0);self.assertEqual(version.value,1)
  self.assertEqual(N.ndb_tx_commit(tx,c.byref(version)),1);self.assertEqual(version.value,0)
 def test_bv_buffer_negotiation_and_empty_value_are_distinct_from_missing(self):
  tx=self.begin();value=bytes(range(256))*512;key=b'large';self.assertEqual(N.ndb_tx_put(tx,buf(key),len(key),buf(value),len(value)),0)
  required,found=U64(),U64();self.assertEqual(N.ndb_tx_get(tx,buf(key),len(key),None,0,c.byref(required),c.byref(found)),3);self.assertEqual((required.value,found.value),(len(value),1))
  data=buf(bytes(len(value)));self.assertEqual(N.ndb_tx_get(tx,buf(key),len(key),data,len(value),c.byref(required),c.byref(found)),0);self.assertEqual(bytes(data),value)
  self.assertEqual(N.ndb_tx_put(tx,None,0,None,0),0);self.assertEqual(N.ndb_tx_get(tx,None,0,None,0,c.byref(required),c.byref(found)),0);self.assertEqual((required.value,found.value),(0,1))
  self.assertEqual(N.ndb_tx_get(tx,buf(b'absent'),6,None,0,c.byref(required),c.byref(found)),0);self.assertEqual((required.value,found.value),(0,0))
 def test_eq_framed_scan_and_argument_rejection(self):
  tx=self.begin();self.assertEqual(N.ndb_tx_put(tx,buf(b'a'),1,buf(b'one'),3),0)
  required,count=U64(),U64();self.assertEqual(N.ndb_tx_scan(tx,None,0,None,0,0,None,0,c.byref(required),c.byref(count)),3)
  self.assertEqual((required.value,count.value),(12,1));output=buf(bytes(12));self.assertEqual(N.ndb_tx_scan(tx,None,0,None,0,0,output,12,c.byref(required),c.byref(count)),0);self.assertEqual(bytes(output),struct.pack('<II',1,3)+b'aone')
  self.assertEqual(N.ndb_tx_get(tx,None,1025,None,0,c.byref(required),c.byref(count)),1)
  self.assertEqual(N.ndb_tx_scan(tx,None,0,None,0,2,None,0,c.byref(required),c.byref(count)),1)
  self.assertEqual(N.ndb_tx_db_info(self.db,None,None),1)
 def test_st_duplicate_owner_and_stale_handle_have_distinct_status(self):
  output=U64(99);self.assertEqual(N.ndb_tx_db_open(buf(self.path),len(self.path),c.byref(output)),23);self.assertEqual(output.value,0)
  tx=self.begin();self.assertEqual(N.ndb_tx_rollback(tx),0);self.assertEqual(N.ndb_tx_put(tx,None,0,None,0),1)
  self.assertEqual(N.ndb_tx_checkpoint(self.db,None,None,None,None),1)
if __name__=='__main__':unittest.main()
