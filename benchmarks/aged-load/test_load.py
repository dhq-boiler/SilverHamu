import unittest
from campaign import handoffs, responses
from audit_load import isolated_acquisitions

def attempt(i, sent, start, finished, seq):
    return i,sent,start,start,start,start,0,0,0,finished,seq

class ArrivalTests(unittest.TestCase):
    def test_due_times_preserve_generator_backlog_and_retries(self):
        origin=1000
        due=origin+50_000_000
        rows=responses([attempt(0,origin+8,origin+10,origin+99,1),
                        attempt(1,due+700,due+800,due+1000,0),
                        attempt(1,due+700,due+1100,due+1600,2)],origin,20,2)
        self.assertEqual(rows[1],(1,due,due+700,due+1600,1,2))
        self.assertEqual(rows[1][3]-rows[1][1],1600)
        self.assertEqual(rows[1][3]-rows[1][2],900)

    def test_continuous_arrivals_start_at_send(self):
        rows=responses([attempt(0,10,10,20,1)],0,0,1)
        self.assertEqual(rows[0][1:4],(10,10,20))

    def test_early_and_missing_arrivals_are_rejected(self):
        with self.assertRaises(AssertionError):
            responses([attempt(0,5,5,9,1)],6,10,1)
        with self.assertRaises(AssertionError):
            responses([attempt(0,5,5,9,1)],5,10,2)

    def test_isolated_acquisition_excludes_waiters_and_release_envelopes(self):
        holds=[(0,1,2,7,9),(1,8,10,12,13),(4,14,15,18,19),(2,22,24,26,27),
               (3,21,28,30,31),(0,33,35,36,37)]
        self.assertEqual(isolated_acquisitions(holds),[1,2])

    def test_idle_handoff_is_zero_and_legacy_busy_check_is_retained(self):
        log="""ATTEMPT,0,0,1,1,2,3,4,6,100,9,1
ATTEMPT,1,20,21,21,22,23,24,26,100,29,2
HOLD,0,2,3,8,9
HOLD,4,10,11,12,13
HOLD,0,22,23,28,29
HOLD,4,30,31,32,33
PUBLICATION,1,10,13,14,16,17,31,31,31,32,32,34,64"""
        account=handoffs(log,2,1,1,allow_idle=True)
        self.assertEqual(account["contended"],0)
        self.assertEqual(account["p99_envelope_us"],0)
        self.assertEqual(account["active_envelope_ms"],0)
        with self.assertRaises(AssertionError):
            handoffs(log,2,1,1)

if __name__=="__main__":unittest.main()
