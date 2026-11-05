import importlib.util,pathlib,unittest
spec=importlib.util.spec_from_file_location('rq_analysis',pathlib.Path(__file__).with_name('analyze.py'));a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
def event(t,kind,slot='R000001',extra='',pid=7,comm='worker'):
 return dict(ns=t,pid=pid,comm=comm,body=f'{kind}: rq={slot}'+extra)
def done(start=0):return [event(start,'rq_start'),event(start+1,'rq_update',extra=' error=0 bytes=4096'),event(start+2,'rq_update_return',extra=' more=0')]
class RequestIdentityTests(unittest.TestCase):
 def test_reuse_gets_a_new_dispatch_attempt(self):
  r=a.requests(done()+done(10));self.assertFalse(r['issues']);self.assertEqual([x['attempt'] for x in r['attempts']],[1,2]);self.assertEqual(r['reused_slots'],1)
 def test_partial_update_does_not_complete_dispatch(self):
  es=[event(0,'rq_start'),event(1,'rq_update',extra=' error=0 bytes=512'),event(2,'rq_update_return',extra=' more=1')]
  r=a.requests(es)['attempts'][0];self.assertNotIn('data_end_ns',r)
  r=a.requests(es+done(10)[1:])['attempts'][0];self.assertEqual(r['data_end_ns'],12)
 def test_requeue_allows_another_attempt_without_completing_the_first(self):
  r=a.requests([event(0,'rq_start'),event(1,'rq_requeue')]+done(10));self.assertFalse(r['issues']);self.assertNotIn('data_end_ns',r['attempts'][0])
 def test_duplicate_start_is_not_silently_paired(self):
  self.assertTrue(a.requests([event(0,'rq_start'),event(1,'rq_start')])['issues'])
 def test_return_without_entry_is_rejected(self):
  self.assertIn('return without entry',a.requests([event(0,'rq_update_return',extra=' more=0')])['issues'])
 def test_incomplete_update_is_detected(self):
  self.assertIn('entry without return',a.requests([event(0,'rq_update',extra=' bytes=1')])['issues'])
 def test_existing_request_is_counted_without_inventing_start(self):
  r=a.requests(done()[1:]);self.assertFalse(r['attempts']);self.assertEqual(r['before_start']['rq_update_return'],1)
 def test_worker_is_not_assigned_by_temporal_overlap(self):
  journal=dict(ns=0,pid=9,comm='jbd2',body='jbd2_start_commit: dev 8,64 transaction 42 sync 0')
  r=a.requests([journal,event(1,'rq_start',pid=9),event(2,'rq_start','R000002',pid=7)])
  self.assertEqual(r['attempts'][0]['journal_context']['transaction'],42);self.assertIsNone(r['attempts'][1]['journal_context'])
 def test_end_hook_is_not_required_for_data_completion(self):
  r=a.requests(done()+[event(3,'rq_free')])['attempts'][0];self.assertEqual(r['ends'],0);self.assertEqual(r['data_end_ns'],2);self.assertEqual(r['free_ns'],3)
 def test_invalid_return_does_not_become_success(self):
  with self.assertRaises(ValueError):a.requests(done()[:2]+[event(2,'rq_update_return',extra=' more=2')])
 def test_completion_cannot_cross_a_dispatch_boundary(self):
  es=[event(0,'rq_update',extra=' error=0 bytes=4096'),event(1,'rq_start'),event(2,'rq_update_return',extra=' more=0')]
  self.assertIn('update crosses dispatch boundary',a.requests(es)['issues'])
 def test_error_completion_preserves_error_status(self):
  es=[event(0,'rq_start'),event(1,'rq_update',extra=' error=5 bytes=4096'),event(2,'rq_update_return',extra=' more=0')]
  self.assertEqual(a.requests(es)['attempts'][0]['updates'][0]['error'],5)
if __name__=='__main__':unittest.main()
