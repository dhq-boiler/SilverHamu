import unittest
from analyze import phases,analyze_case
class AnalysisTests(unittest.TestCase):
    def test_eq_phases_sum_without_double_counting(self):
        item=dict(commit_ok=True,trace=[10,20,21,30,40,50,70,80],detail=[9,15,32,33,72,85,90,8])
        row=dict(start_ns=5,end_ns=95,sent_ns=100)
        p=phases(row,item,60);self.assertAlmostEqual(sum(p.values()),95/1e6)
        self.assertEqual(p['sync'],10/1e6);self.assertEqual(p['before_sync'],10/1e6)
        item['detail'][1]=8
        p=phases(row,item,60);self.assertEqual(p['leader_wait'],0);self.assertEqual(p['collection'],10/1e6)
    def test_er_out_of_order_and_missing_phase_rejected(self):
        item=dict(commit_ok=True,trace=[10,20,21,30,40,50,70,80],detail=[9,15,32,33,72,85,90,8])
        with self.assertRaises(AssertionError):phases(dict(start_ns=5,end_ns=95,sent_ns=100),item,75)
        item['trace'][2]=0
        with self.assertRaises(AssertionError):phases(dict(start_ns=5,end_ns=95,sent_ns=100),item,60)
    def test_er_dropped_records_never_silently_accepted(self):
        with self.assertRaises(AssertionError):analyze_case({},[],[dict(worker=0,dropped=1,records=[])])
if __name__=='__main__':unittest.main()
