import unittest
from analyze import tail

def record(seq,planned,ms):
    return dict(seq=seq,kind='write',status='ok',planned=planned,enqueued=planned,began=planned+1,ended=planned+int(ms*1e6))

class Tests(unittest.TestCase):
    def test_BV_thresholds_and_startup_partition_include_the_exact_boundary(self):
        rows=[record(0,0,9.999),record(1,999999999,10),record(2,1000000000,20),record(3,2000000000,50)]
        x=tail(rows,0)['write']
        self.assertEqual(x['at_least_ms'],{'10':3,'20':2,'50':1})
        self.assertEqual(x['first_second_ms']['count'],2);self.assertEqual(x['later_ms']['count'],2)
    def test_ER_duplicate_requests_are_rejected(self):
        r=record(0,0,1)
        with self.assertRaises(AssertionError):tail([r,r],0)
    def test_ER_reversed_timestamps_are_rejected(self):
        r=record(0,0,1);r['began']=r['ended']+1
        with self.assertRaises(AssertionError):tail([r],0)
    def test_BV_empty_kind_is_not_reported_as_zero_latency(self):
        self.assertIsNone(tail([],0)['write']['call_ms']['p99'])
    def test_ER_refused_requests_stay_in_denominator(self):
        r=record(0,0,1);r['status']='queue_full';r.pop('began')
        x=tail([r,record(1,1,2)],0)['write'];self.assertEqual((x['requested'],x['completed']),(2,1))
if __name__=='__main__':unittest.main()
