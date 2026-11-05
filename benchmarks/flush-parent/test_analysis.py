import unittest
from analyze import parents,quality
def e(t,body,pid=7,cpu=0):return dict(ns=t,body=body,pid=pid,cpu=cpu,comm='test')
def journal():return e(0,'jbd2_start_commit: dev 8,64 transaction 42')
def parent(slot='R1',t=1):return e(t,'fp_insert: rq='+slot)
def start():return e(2,'rq_start: rq=R9',9)
def entry(t=3):return e(t,'fp_end: rq=R9 error=0',9)
def member(slot='R1',t=4,seq=1):return e(t,f'fp_seq: rq={slot} fq=R8 seq={seq} error=0',9)
def end(t=5):return e(t,'fp_return: rq=R9',9)
class ParentTests(unittest.TestCase):
 def test_multiple_parents_share_one_callback(self):
  a=parents([journal(),parent(),parent('R2'),start(),entry(),member(),member('R2'),end()]);self.assertFalse(a['issues']);self.assertEqual(len(a['callbacks'][0]['members']),2)
 def test_context_is_preserved_across_worker_handoff(self):
  a=parents([journal(),parent(),start(),entry(),member(),end()]);self.assertEqual(a['callbacks'][0]['members'][0]['parent']['journal']['transaction'],42)
 def test_preexisting_parent_is_unknown(self):
  a=parents([start(),entry(),member(),end()]);self.assertIsNone(a['callbacks'][0]['members'][0]['parent'])
 def test_reused_slot_has_distinct_insertion(self):
  a=parents([journal(),parent(),parent(t=2),start(),entry(),member(),end()]);self.assertEqual(a['callbacks'][0]['members'][0]['parent']['insertion'],2)
 def test_free_removes_old_attribution(self):
  a=parents([journal(),parent(),e(2,'rq_free: rq=R1'),start(),entry(),member(),end()]);self.assertIsNone(a['callbacks'][0]['members'][0]['parent'])
 def test_unrelated_thread_is_not_assigned(self):
  a=parents([journal(),e(1,'fp_insert: rq=R1',8),start(),entry(),member(),end()]);self.assertIsNone(a['callbacks'][0]['members'][0]['parent']['journal'])
 def test_missing_return_is_reported(self):self.assertTrue(parents([entry()])['issues'])
 def test_unmatched_return_is_reported(self):self.assertTrue(parents([end()])['issues'])
 def test_duplicate_members_are_reported(self):self.assertTrue(parents([entry(),member(),member(),end()])['issues'])
 def test_other_cpu_cannot_attach_member(self):
  a=parents([entry(),e(4,'fp_seq: rq=R1 fq=R8 seq=1 error=0',9,1),end()]);self.assertFalse(a['callbacks'][0]['members'])
 def test_data_phase_is_not_flush_membership(self):
  a=parents([entry(),member(seq=2),end()]);self.assertFalse(a['callbacks'][0]['members'])
 def test_reference_release_callback_is_not_completion(self):
  a=parents([start(),entry(),end(),entry(6),member(t=7,seq=4),end(8)]);self.assertFalse(a['callbacks'][0]['members']);self.assertEqual(a['callbacks'][0]['dispatch'],a['callbacks'][1]['dispatch']);self.assertEqual(len(a['callbacks'][1]['members']),1)
 def test_errors_are_retained(self):
  a=parents([entry(),e(4,'fp_seq: rq=R1 fq=R8 seq=4 error=1',9),end()]);self.assertEqual(a['callbacks'][0]['members'][0]['error'],1)
 def test_missing_or_missed_probe_rejects_capture(self):
  m=dict(cleaned=True,request_probe_count=10,probe_profile=['probe 1 0']*10,stats={'cpu0':'overrun: 0\ncommit overrun: 0\ndropped events: 0'})
  self.assertFalse(quality(m));m['probe_profile'][0]='probe 1 1';self.assertTrue(quality(m));m['probe_profile']=[];self.assertTrue(quality(m))
 def test_ring_loss_and_missing_counters_reject_capture(self):
  m=dict(cleaned=True,request_probe_count=10,probe_profile=['probe 1 0']*10,stats={'cpu0':'overrun: 1\ncommit overrun: 0\ndropped events: 0'})
  self.assertTrue(quality(m));m['stats']={};self.assertTrue(quality(m))
if __name__=='__main__':unittest.main()
