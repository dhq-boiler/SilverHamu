import importlib.util,pathlib,unittest
spec=importlib.util.spec_from_file_location('journal_analysis',pathlib.Path(__file__).with_name('analyze.py'));a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
def e(t,b,pid=7):return dict(ns=t,body=b,pid=pid)
def phase(t,p,tx=1):return e(t,f'jbd2_{p}: dev 8,64 transaction {tx} sync 0')
def timeline():return [phase(0,'start_commit'),phase(1,'commit_locking'),phase(2,'commit_flushing'),phase(3,'commit_logging'),phase(100,'end_commit')]
class JournalAttributionTests(unittest.TestCase):
 def test_complete_phases_partition_scheduled_time(self):
  r=a.journals(timeline(),0,100);self.assertFalse(r['issues']);self.assertEqual(r['transactions'][0]['scheduled_ns'],100)
 def test_sleep_and_execution_restart_are_distinct(self):
  es=timeline()[:-1]+[e(10,'sched_switch: prev_pid=7 prev_state=D ==> next_pid=0'),e(80,'sched_wakeup: comm=jbd2/sde-8 pid=7 prio=120',0),e(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=7',0),timeline()[-1]]
  r=a.journals(es,0,100)['transactions'][0];self.assertEqual((r['blocked_ns'],r['runnable_ns'],r['scheduled_ns']),(70,10,20));self.assertNotIn('io_ns',r)
 def test_missing_wakeup_is_invalid(self):
  es=timeline()[:-1]+[e(10,'sched_switch: prev_pid=7 prev_state=D ==> next_pid=0'),e(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=7',0),timeline()[-1]]
  self.assertIn('missing wakeup',a.journals(es,0,100)['issues'])
 def test_runnable_preemption_does_not_require_wakeup(self):
  es=timeline()[:-1]+[e(10,'sched_switch: prev_pid=7 prev_state=R+ ==> next_pid=0'),e(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=7',0),timeline()[-1]]
  r=a.journals(es,0,100);self.assertFalse(r['issues']);self.assertEqual(r['transactions'][0]['runnable_ns'],80)
 def test_incomplete_commit_is_not_reported_as_completed(self):
  r=a.journals(timeline()[:-1],0,100);self.assertFalse(r['transactions']);self.assertIn('incomplete journal transaction',r['issues'])
 def test_wrong_transaction_is_rejected(self):
  with self.assertRaisesRegex(ValueError,'identity'):a.journals(timeline()[:-1]+[phase(100,'end_commit',2)],0,100)
 def test_missing_phase_is_rejected(self):
  with self.assertRaisesRegex(ValueError,'phase'):a.journals([timeline()[0],timeline()[-1]],0,100)
 def test_window_includes_start_boundary_and_follows_end(self):
  self.assertEqual(len(a.journals(timeline(),0,0)['transactions']),1)
  self.assertEqual(len(a.journals(timeline(),1,100)['transactions']),0)
 def test_nested_commit_is_rejected(self):
  with self.assertRaisesRegex(ValueError,'overlapping'):a.journals([phase(0,'start_commit'),phase(1,'start_commit',2)],0,100)
if __name__=='__main__':unittest.main()
