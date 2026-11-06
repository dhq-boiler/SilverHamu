import copy,unittest
from analyze import partition,validate

def e(t,body,pid=42):return dict(ns=t,body=body,pid=pid)
def example():return [e(0,'sys_fsync(fd: 5)'),e(10,'sched_switch: prev_pid=42 prev_state=D ==> next_pid=0'),e(70,'sched_wakeup: comm=catdb-client pid=42 prio=120',0),e(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=42',0),e(100,'sys_fsync -> 0x0')]
class Tests(unittest.TestCase):
    def test_EQ_partition_is_disjoint_and_sums_to_the_same_sync(self):
        r=partition(example(),0,100,{42},5)[0]
        self.assertEqual((r['scheduled_ns'],r['runnable_ns'],r['blocked_ns']),(20,20,60))
    def test_ST_preemption_needs_no_wakeup(self):
        a=[e(0,'sys_fsync(fd: 5)'),e(10,'sched_switch: prev_pid=42 prev_state=R+ ==> next_pid=0'),e(90,'sched_switch: prev_pid=0 prev_state=R ==> next_pid=42',0),e(100,'sys_fsync -> 0x0')]
        self.assertEqual(partition(a,0,100,{42},5)[0]['runnable_ns'],80)
    def test_ER_missing_wakeup_rejects_attribution(self):
        with self.assertRaises(AssertionError):partition(example()[:2]+example()[3:],0,100,{42},5)
    def test_BV_exact_boundaries_and_zero_duration(self):
        self.assertEqual(partition([e(10,'sys_fsync(fd: 5)'),e(10,'sys_fsync -> 0x0')],10,10,{42},5)[0]['duration_ns'],0)
    def test_ER_wrong_fd_or_thread_cannot_be_attributed(self):
        with self.assertRaises(AssertionError):partition(example(),0,100,{42},6)
        self.assertEqual(partition(example(),0,100,{43},5),[])
    def test_ER_incomplete_failed_and_nested_sync_are_rejected(self):
        for a in [example()[:-1],[e(0,'sys_fsync(fd: 5)'),e(1,'sys_fsync -> 0xfffffffb')],[e(0,'sys_fsync(fd: 5)'),e(1,'sys_fsync(fd: 5)')]]:
            with self.assertRaises(AssertionError):partition(a,0,100,{42},5)
    def test_ER_loss_count_and_capture_bounds_are_mandatory(self):
        meta=dict(cleaned=True,clock='mono',start_ns=0,end_ns=100,stats={'cpu0':'overrun: 0\ncommit overrun: 0\ndropped events: 0'},tids=[42],wal_fd=5)
        m=dict(start_ns=0,finish_ns=100,sync_attempts=1)
        self.assertEqual(len(validate(meta,m,example())),1)
        for field,value in [('cleaned',False),('end_ns',99),('stats',{'cpu0':'overrun: 1\ncommit overrun: 0\ndropped events: 0'})]:
            bad=copy.deepcopy(meta);bad[field]=value
            with self.assertRaises(AssertionError):validate(bad,m,example())
        with self.assertRaises(AssertionError):validate(meta,{**m,'sync_attempts':2},example())
if __name__=='__main__':unittest.main()
