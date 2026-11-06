import unittest
from analyze import inspect
def fixture():
    return {'bounds':[[10,20,100]],'events':[[0,3,4,0,1,1,0,21,30],[0,1,4,-1,0,0,0,40,90]],'fds':{'4':{'path':'catdb.wal','size':0}},'samples_ns':[90],'diagnostic':True,'engine':'catdb-embedded','final_sizes':{'catdb.wal':1}}
class Perspectives(unittest.TestCase):
    def test_eq_exact_partition_and_growth(self):
        r=inspect(fixture());p=r['phases'][0]
        self.assertEqual(p['sync_ns']+p['write_ns']+p['truncate_ns']+p['residual_ns'],90)
        self.assertEqual(r['extension_bytes'],1)
    def test_er_missing_sync_rejected(self):
        d=fixture();d['events'].pop()
        with self.assertRaises(AssertionError):inspect(d)
    def test_er_nested_or_unmatched_events_rejected(self):
        for e in [[0,1,4,-1,0,0,0,25,90],[1,1,4,-1,0,0,0,40,90],[0,1,4,-1,0,0,0,40,101]]:
            d=fixture();d['events'][1]=e
            with self.assertRaises(AssertionError):inspect(d)
    def test_er_unobserved_extension_and_failed_write_rejected(self):
        d=fixture();d['final_sizes']['catdb.wal']=2
        with self.assertRaises(AssertionError):inspect(d)
        d=fixture();d['events'][0][5]=-1;d['events'][0][6]=28
        with self.assertRaises(AssertionError):inspect(d)
    def test_st_rewind_and_truncate(self):
        d=fixture();d['engine']='sqlite';d['fds']['4']={'path':'sqlite.db-wal','size':10};d['final_sizes']={'sqlite.db-wal':1}
        d['events']=[[0,4,4,8,1,1,0,21,25],[0,4,4,0,1,1,0,26,30],[0,5,4,1,0,0,0,31,35],[0,2,4,-1,0,0,0,40,90]]
        r=inspect(d);self.assertEqual(r['wal_rewind_observations'],1);self.assertEqual(r['extension_bytes'],0)
if __name__=='__main__':unittest.main()
