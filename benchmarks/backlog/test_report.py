import copy
import unittest
from report import validate,recovery

class ReportTests(unittest.TestCase):
    def fixture(self):
        r=dict(attempted=2,completed=2,rejected=0,errors=[],clients=1,rate=10,scenario='normal',sync_attempts=1,injected_stalls=0,
               schedule=dict(origin_ns=1,batch_limit=8,outstanding_capacity=2,
                    counters=dict(before=dict(version='4',sync_attempts=1),after=dict(version='6',sync_attempts=2))))
        es=[dict(id=i,worker=0,sequence=i,planned_ns=1+i*100_000_000,admitted_ns=2+i*100_000_000,
                 sent_ns=100_000_005,finished_ns=100_000_020,status='completed',batch_id=0,key_index=i) for i in range(2)]
        batches=[dict(id=0,worker=0,size=2,sent_ns=100_000_005,finished_ns=100_000_020)]
        return r,es,batches
    def test_eq_multiple_outcomes_share_one_operation_without_losing_identity(self):
        r,e,b=self.fixture();groups=validate(r,e,b)
        self.assertEqual(len(groups[(0,0)]),2)
    def test_bv_wrong_batch_identity_capacity_and_duplicate_key_are_rejected(self):
        for case in ['key','batch','capacity','version','schedule']:
            r,e,b=self.fixture()
            if case=='key':e[1]['key_index']=0
            elif case=='batch':e[1]['batch_id']=1
            elif case=='capacity':r['schedule']['outstanding_capacity']=1
            elif case=='version':r['schedule']['counters']['after']['version']='7'
            else:e[0]['planned_ns']+=1
            with self.assertRaises((AssertionError,KeyError)):validate(r,e,b)
    def recovery_fixture(self):
        r=dict(scenario='single',schedule=dict(armed_client_ns=0,armed_done_ns=10,
               counters=dict(after=dict(injected_stall_ns=[0,100_000_000,100_000_000]))))
        es=[dict(planned_ns=i*1_000_000,sent_ns=i*1_000_000,status='completed') for i in range(300)]
        return r,es
    def test_st_recovery_requires_a_full_stable_window_after_actual_injection_end(self):
        r,e=self.recovery_fixture();result=recovery(r,e)
        self.assertTrue(result['recovered']);self.assertAlmostEqual(result['recovery_ms'],.999995)
        e[150]['status']='rejected'
        result=recovery(r,e);self.assertAlmostEqual(result['recovery_ms'],50.999995)
    def test_bv_delay_threshold_is_inclusive_and_unrecovered_is_not_zero(self):
        r,e=self.recovery_fixture()
        for x in e:x['sent_ns']+=5_000_000
        self.assertTrue(recovery(r,e)['recovered'])
        for x in e:x['sent_ns']+=1
        result=recovery(r,e);self.assertFalse(result['recovered']);self.assertNotIn('recovery_ms',result)
    def test_normal_case_does_not_invent_an_injection_end(self):
        self.assertIsNone(recovery(dict(scenario='normal'),[]))

if __name__=='__main__':unittest.main()
