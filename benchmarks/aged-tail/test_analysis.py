import unittest
from analyze import phases,requests

class Tests(unittest.TestCase):
    def diag(self):
        return {'status':'ok','diagnostic':{'commit_ok':True,'trace':[10,20,30,40,60,70,80,90],'detail':[5,15,45,50,85,95,96,1],'costs':[2,3],'finished_ns':100,'dispatch_ns':99}}
    def test_EQ_partition_matches_dispatch_without_summing_unrelated_percentiles(self):
        p=phases(self.diag());self.assertAlmostEqual(sum(p.values()),99/1e6);self.assertEqual(p['sync_envelope'],10/1e6)
    def test_BV_joining_an_existing_collection_has_no_negative_queue_wait(self):
        for begin,queue,collection in [(5,0,10),(10,0,10),(20,10,0)]:
            r=self.diag();r['diagnostic']['detail'][1]=begin
            p=phases(r);self.assertEqual(p['wait_before_collection'],queue/1e6);self.assertEqual(p['collection_wait'],collection/1e6)
        r=self.diag();r['diagnostic']['detail'][1]=21
        with self.assertRaises(AssertionError):phases(r)
    def test_ER_missing_reversed_and_invalid_costs_are_rejected(self):
        for field,value in [('trace',[0]*8),('costs',[9,9]),('finished_ns',80)]:
            r=self.diag();r['diagnostic'][field]=value
            with self.assertRaises(AssertionError):phases(r)
        with self.assertRaises(KeyError):phases({'status':'ok'})
    def test_BV_exact_10_20_ms_and_rejected_requests(self):
        r=[{'seq':i,'kind':'write','status':'ok','planned':0,'enqueued':0,'began':1,'ended':n} for i,n in enumerate([9999999,10000000,19999999,20000000])]
        r.append({'seq':4,'kind':'write','status':'queue_full'})
        s=requests(r)['write'];self.assertEqual(s['over10ms'],3);self.assertEqual(s['over20ms'],1);self.assertEqual(s['queue_refused'],1)
    def test_ER_duplicate_requests_and_reversed_clock_rejected(self):
        r={'seq':0,'kind':'write','status':'ok','planned':0,'enqueued':2,'began':1,'ended':3}
        with self.assertRaises(AssertionError):requests([r])
        with self.assertRaises(AssertionError):requests([r,r])

if __name__=='__main__':unittest.main()
