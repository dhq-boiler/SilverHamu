import unittest
from partition import allocate,stack_kind,syscall_segments

class TraceTests(unittest.TestCase):
    def test_sleep_wake_and_preempt_are_disjoint(self):
        events=[(t,0,81,k) for t,k in [(0,"enter"),(1,"journal"),(6,"wake"),(8,"in"),(9,"preempt"),(12,"in"),(13,"exit")]]
        self.assertEqual(syscall_segments(events)[0][0],[(0,1,"scheduled"),(1,6,"journal"),(6,8,"wake_wait"),(8,9,"scheduled"),(9,12,"preempt_wait"),(12,13,"scheduled")])
        for bad in (events[4:],events[:-1],events[:2]+events[3:],events+[(15,0,81,"exit")]):
            with self.assertRaises(AssertionError):syscall_segments(bad)

    def test_wakeup_before_switch_out_is_not_runnable_delay(self):
        from collections import Counter
        observed=Counter()
        events=[(t,0,81,k) for t,k in [(0,"enter"),(1,"wake"),(2,"journal"),(5,"wake"),(6,"in"),(8,"exit")]]
        self.assertEqual(syscall_segments(events,observed)[0][0],[(0,2,"scheduled"),(2,5,"journal"),(5,6,"wake_wait"),(6,8,"scheduled")])
        self.assertEqual(observed["wake_while_scheduled"],1)
        with self.assertRaises(AssertionError):syscall_segments(events[:3]+events[4:])
    def test_stack_evidence_required_for_io_path(self):
        self.assertEqual(stack_kind(["schedule","ext4_sync_file"]),"blocked_unknown")
        self.assertEqual(stack_kind(["io_schedule","folio_wait_writeback"]),"writeback")
        self.assertEqual(stack_kind(["jbd2_log_wait_commit"]),"journal")
        self.assertEqual(stack_kind(["io_schedule"]),"io_other")

    def test_phase_crossing_clock_and_thread_matching(self):
        lines=["OS_CLOCK,0,1000,1000"]*2
        lines.append("PUBLICATION,1,0,4,5,8,11,11,12,12,13,13,14,100")
        events=[]
        for i in range(4):
            lines.extend([f"OS_LANE,{i},{10+i},cw0123456789{i}",f"ATTEMPT,{i},0,0,0,0,0,1,16,120,17,{i+1}"])
            events.extend((1000+t,i,80+i,k) for t,k in [(2,"enter"),(3,"writeback"),(10,"wake"),(12,"in"),(15,"exit")])
        log="\n".join(lines)
        b,e,details,_=allocate(log,events)
        self.assertEqual((sum(b.values()),e,len(details)),(60,0,4))
        self.assertEqual(b["snapshot_write","writeback"],12)
        self.assertEqual(b["snapshot_sync","wake_wait"],4)
        self.assertEqual(sum(d["outside_syscall"] for d in details),8)
        with self.assertRaises(AssertionError):allocate(log,events[:-1])
        with self.assertRaises(AssertionError):allocate(log.replace("1000,1000","9000,9000"),events)
        with self.assertRaises(AssertionError):allocate(log,[(t,l,80,k) for t,l,_,k in events])

if __name__=="__main__":unittest.main()
