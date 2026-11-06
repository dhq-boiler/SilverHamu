import importlib.util,pathlib,unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
s=importlib.util.spec_from_file_location('fixtures',ROOT/'benchmarks/resource-envelope/test_client.py');f=importlib.util.module_from_spec(s);s.loader.exec_module(f)
s=importlib.util.spec_from_file_location('ramp',pathlib.Path(__file__).with_name('ramp_client.py'));m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Tests(unittest.TestCase):
 def row(self,p99=1,refused=0,errors=None):return {'p99_ms':p99,'queue_rejected':refused,'errors':errors or []}
 def test_BV_first_stage_and_double_boundary(self):
  self.assertIsNone(m.stop_reason(None,self.row(100)))
  for x in [1.999,2]:self.assertIsNone(m.stop_reason(1,self.row(x)))
  self.assertEqual(m.stop_reason(1,self.row(2.001)),'p99_more_than_doubled')
 def test_ER_refusal_or_error_ends_ramp(self):
  self.assertEqual(m.stop_reason(None,self.row(refused=1)),'queue_refusal')
  self.assertEqual(m.stop_reason(1,self.row(errors=['failure'])),'operation_error')
if __name__=='__main__':unittest.main()
