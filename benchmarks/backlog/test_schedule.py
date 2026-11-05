import collections
import time
import unittest
from schedule import execute

class ScheduleTests(unittest.TestCase):
    def factory(self,delay=0,duplicate=False,fail=False):
        class Adapter:
            def warm(self):pass
            def arm(self):pass
            def counters(self):return {}
            def identity(self,e):return e['id']//2 if duplicate else e['id']
            def update_many(self,events):
                if delay:time.sleep(delay)
                if fail and events[0]['id']==0:raise OSError('injected failure')
                return [{'ok':True} for _ in events]
            def close(self):pass
        return lambda w:Adapter()
    def test_eq_st_backlog_batches_only_ready_distinct_keys_without_reordering(self):
        r=execute(self.factory(.002,True),100,1,100000,batch_size=8,capacity=64)
        es=[e for e in r['events'] if e['status']=='completed']
        self.assertTrue(any(b['size']>1 for b in r['batches']))
        groups=collections.defaultdict(list)
        for e in es:groups[e['batch_id']].append(e)
        for group in groups.values():
            self.assertEqual(len(group),len({e['id']//2 for e in group}))
        self.assertEqual([e['id'] for group in groups.values() for e in group],sorted(e['id'] for e in es))
    def test_bv_outstanding_budget_includes_queue_carry_and_active_batch(self):
        for batch in [1,8]:
            r=execute(self.factory(.01),200,4,100000,batch_size=batch,capacity=16)
            changes=[]
            for e in r['events']:
                if e['status']=='completed':changes.extend([(e['admitted_ns'],1),(e['finished_ns'],-1)])
            outstanding=0
            for _,delta in sorted(changes):
                outstanding+=delta;self.assertLessEqual(outstanding,16)
            self.assertEqual(outstanding,0)
            self.assertTrue(any(e['status']=='rejected' for e in r['events']))
    def test_st_low_load_does_not_wait_to_fill_a_batch(self):
        r=execute(self.factory(),4,1,20,batch_size=8)
        self.assertEqual([b['size'] for b in r['batches']],[1]*4)
    def test_er_failed_batch_does_not_strand_later_work(self):
        r=execute(self.factory(fail=True),8,1,50,batch_size=8)
        self.assertEqual(sum(e['status']=='failed' for e in r['events']),1)
        self.assertEqual(sum(e['status']=='completed' for e in r['events']),7)
    def test_st_fixed_deadlines_are_not_shifted_by_backlog(self):
        r=execute(self.factory(.003),20,1,10000,batch_size=8)
        self.assertEqual([e['planned_ns'] for e in r['events']],[r['origin_ns']+i*100000 for i in range(20)])
    def test_bv_invalid_inputs_open_no_connections(self):
        def factory(w):self.fail('unexpected connection')
        for args in [(0,1,200,8,64),(8,3,200,8,64),(8,1,200,9,64),(8,4,200,8,3)]:
            with self.assertRaises(ValueError):execute(factory,*args)

if __name__=='__main__':unittest.main()
