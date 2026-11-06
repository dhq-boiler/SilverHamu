import importlib.util,pathlib,unittest
spec=importlib.util.spec_from_file_location('solo_report',pathlib.Path(__file__).with_name('report.py'))
report=importlib.util.module_from_spec(spec);spec.loader.exec_module(report)
class PhaseTests(unittest.TestCase):
    def record(self):
        return {'entered':10,'commit':20,'ended':100,'sync_start':55,'trace':[20,35,36,37,40,50,80,90],'detail':[20,25,37,38,81,92,95,20]}
    def test_EQ_phases_sum_to_the_same_request_including_clock_edges(self):
        p=report.phases(self.record(),True,92)
        self.assertEqual(p['collection'],10);self.assertEqual(p['sync'],25)
        self.assertEqual(p['clock_edges'],2);self.assertEqual(sum(p.values()),92)
    def test_BV_ungrouped_front_is_not_misreported_as_a_collection_wait(self):
        p=report.phases(self.record(),False,90)
        self.assertEqual(p['collection'],0);self.assertEqual(p['commit_front'],20)
    def test_ER_out_of_order_or_impossible_duration_is_rejected(self):
        d=self.record();d['sync_start']=49
        with self.assertRaises(AssertionError):report.phases(d,True,92)
        with self.assertRaises(AssertionError):report.phases(self.record(),True,89)
        d=self.record();d['detail'][1]=0
        with self.assertRaises(AssertionError):report.phases(d,True,92)
if __name__=='__main__':unittest.main()
