import unittest
from identity import target_ids,exportable
class Tests(unittest.TestCase):
    def test_EQ_namespace_id_is_not_used_as_kernel_id(self):
        line='as-123-26071 [002] ..... 1.0: sys_fsync(fd: 5)'
        self.assertEqual(target_ids(line,'as-123'),{26071});self.assertTrue(exportable(line,{26071}));self.assertFalse(exportable(line,{11783}))
    def test_ST_switch_fields_identify_target_with_unknown_prefix(self):
        line='<...>-9 [001] ..... 1.0: sched_switch: prev_comm=other prev_pid=9 prev_state=R ==> next_comm=as-123 next_pid=42'
        self.assertEqual(target_ids(line,'as-123'),{42});self.assertTrue(exportable(line,{42}))
        self.assertTrue(exportable('<...>-42 [001] ..... 1.1: sys_fsync(fd: 5)',{42}))
    def test_ER_prefix_collisions_and_unrelated_calls_are_excluded(self):
        self.assertEqual(target_ids('as-1234-42 [001] ..... 1.0: sys_fsync(fd: 5)','as-123'),set())
        self.assertFalse(exportable('other-9 [001] ..... 1.0: sys_fsync(fd: 5)',{42}))
if __name__=='__main__':unittest.main()
