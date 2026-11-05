import queue
import threading
import time
import unittest
from schedule import admit,execute,planned_at

class ScheduleTests(unittest.TestCase):
    def test_fixed_deadlines_do_not_depend_on_previous_completion(self):
        self.assertEqual([planned_at(100,n,250) for n in range(4)],[100,4_000_100,8_000_100,12_000_100])
        self.assertEqual(planned_at(0,8000,250),32_000_000_000)
        for rate in [0,-1]:
            with self.assertRaises(ValueError):planned_at(0,1,rate)

    def test_capacity_boundary_rejects_without_overwriting_admitted_work(self):
        q=queue.Queue(1);first={'admitted_ns':1};second={'admitted_ns':2}
        self.assertTrue(admit(q,first));self.assertFalse(admit(q,second))
        self.assertIs(q.get_nowait(),first)
        self.assertEqual(second,{'admitted_ns':2,'finished_ns':2,'status':'rejected'})

    def factory(self,delay=0,fail=False):
        class Adapter:
            def warm(self):pass
            def counters(self):return {'sync_attempts':0,'version':'0'}
            def update(self,event):
                if delay:time.sleep(delay)
                if fail and event['id']==0:raise OSError('injected failure')
            def probe(self,event):return None
            def close(self):pass
        return lambda w:Adapter()

    def test_overload_drains_all_admitted_work_and_accounts_for_every_rejection(self):
        r=execute(self.factory(delay=.01),100,4,100_000,4)
        self.assertEqual(len(r['events']),100)
        self.assertTrue(any(e['status']=='rejected' for e in r['events']))
        self.assertTrue(all(n<=1 for n in r['queue_high_water']))
        self.assertTrue(all(e['status'] in ('completed','rejected') for e in r['events']))
        for e in r['events']:
            self.assertEqual(e['planned_ns'],planned_at(r['origin_ns'],e['id'],100_000))
            if e['status']=='completed':
                self.assertLessEqual(e['planned_ns'],e['admitted_ns'])
                self.assertLessEqual(e['admitted_ns'],e['sent_ns'])
                self.assertLessEqual(e['sent_ns'],e['finished_ns'])

    def test_operation_failure_is_recorded_and_remaining_requests_complete(self):
        r=execute(self.factory(fail=True),8,4,200,64)
        self.assertEqual(sum(e['status']=='failed' for e in r['events']),1)
        self.assertEqual(sum(e['status']=='completed' for e in r['events']),7)

    def test_connection_initialization_failure_does_not_strand_other_workers(self):
        def factory(w):
            if w==0:raise OSError('injected startup failure')
            return self.factory()(w)
        with self.assertRaisesRegex(RuntimeError,'worker failure'):execute(factory,8,4,200)

    def test_invalid_workload_and_capacity_are_rejected_before_opening_connections(self):
        def factory(w):self.fail('opened a connection for invalid input')
        for args in [(0,1,200,64),(8,2,200,64),(9,4,200,64),(8,4,0,64),(8,4,200,3),(8,4,200,5)]:
            with self.assertRaises(ValueError):execute(factory,*args)

if __name__=='__main__':unittest.main()
