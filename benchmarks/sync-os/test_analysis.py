import gzip,importlib.util,pathlib,tempfile,unittest
spec=importlib.util.spec_from_file_location('analysis',pathlib.Path(__file__).with_name('analyze.py'))
analysis=importlib.util.module_from_spec(spec);spec.loader.exec_module(analysis)
class SynchronizationPartitionTests(unittest.TestCase):
 def measure(self,lines,start=0,end=10**12):
  with tempfile.TemporaryDirectory() as d:
   p=pathlib.Path(d)/'trace.gz'
   with gzip.open(p,'wt') as f:f.write('\n'.join(lines))
   return analysis.analyze(p,start,end)
 def line(self,t,event,pid=42):return f'catdb-client-{pid} [001] ..... 1.{t:06d}: {event}'
 def test_io_sleep_and_wakeup_delay_are_distinct(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(10,'sched_switch: prev_pid=42 prev_state=D ==> next_pid=0'),self.line(11,'<stack trace>'),' => io_schedule',self.line(80,'sched_wakeup: comm=catdb-client pid=42 prio=120',0),self.line(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=42',0),self.line(100,'sys_fsync -> 0x0')])
  self.assertFalse(a['issues']);r=a['syncs'][0]
  self.assertEqual((r['scheduled_ns'],r['runnable_ns'],r['blocked_ns'],r['io_stack_blocked_ns']),(20000,10000,70000,70000))
 def test_uninterruptible_lock_is_not_classified_as_io(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(10,'sched_switch: prev_pid=42 prev_state=D ==> next_pid=0'),self.line(11,'<stack trace>'),' => rwsem_down_read_slowpath',self.line(80,'sched_wakeup: comm=catdb-client pid=42 prio=120',0),self.line(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=42',0),self.line(100,'sys_fsync -> 0x0')])
  self.assertEqual(a['syncs'][0]['io_stack_blocked_ns'],0)
  self.assertEqual(a['syncs'][0]['other_blocked_ns'],70000)
 def test_preemption_is_runnable_without_wakeup(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(10,'sched_switch: prev_pid=42 prev_state=R+ ==> next_pid=1'),self.line(90,'sched_switch: prev_pid=1 prev_state=S ==> next_pid=42',1),self.line(100,'sys_fsync -> 0x0')])
  self.assertEqual(a['syncs'][0]['runnable_ns'],80000);self.assertFalse(a['issues'])
 def test_missing_wakeup_invalidates_attribution(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(10,'sched_switch: prev_pid=42 prev_state=D ==> next_pid=0'),self.line(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=42',0),self.line(100,'sys_fsync -> 0x0')])
  self.assertIn('scheduled without wakeup',a['issues'])
 def test_truncated_sync_is_not_a_completed_sample(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)')]);self.assertFalse(a['syncs']);self.assertIn('incomplete fsync',a['issues'])
 def test_unrelated_thread_is_excluded(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)').replace('catdb-client','unrelated'),self.line(100,'sys_fsync -> 0x0')]);self.assertFalse(a['syncs'])
 def test_zero_duration_is_a_complete_call(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(0,'sys_fsync -> 0x0')]);self.assertEqual(a['syncs'][0]['duration_ns'],0)
 def test_concurrent_threads_have_independent_partitions(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(10,'sys_fsync(fd: 5)',43),self.line(20,'sys_fsync -> 0x0',43),self.line(30,'sys_fsync -> 0x0')])
  self.assertEqual({r['tid']:r['duration_ns'] for r in a['syncs']},{43:10000,42:30000})
 def test_failed_sync_return_is_preserved(self):
  a=self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(100,'sys_fsync -> 0xfffffffb')]);self.assertEqual(a['syncs'][0]['return'],'0xfffffffb')
 def test_nested_entry_is_rejected(self):
  with self.assertRaisesRegex(ValueError,'nested'):self.measure([self.line(0,'sys_fsync(fd: 5)'),self.line(10,'sys_fsync(fd: 5)')])
 def test_workload_window_includes_boundaries_and_follows_return(self):
  lines=[]
  for begin,finish in [(0,5),(10,15),(20,25),(30,40)]:
   lines += [self.line(begin,'sys_fsync(fd: 5)'),self.line(finish,'sys_fsync -> 0x0')]
  a=self.measure(lines,1_000_010_000,1_000_020_000)
  self.assertEqual([r['start_ns'] for r in a['syncs']],[1_000_010_000,1_000_020_000]);self.assertFalse(a['issues'])
if __name__=='__main__':unittest.main()
