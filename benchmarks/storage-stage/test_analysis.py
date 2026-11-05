import struct,unittest
from analyze import stages,join
from btf import request_size
def e(t,k,slot='R1',extra='',pid=1,cpu=0):return dict(ns=t,body=f'{k}: rq={slot}{extra}',pid=pid,cpu=cpu,comm='test')
def sequence():return [e(0,'rq_start'),e(1,'st_queue'),e(2,'st_queue_return',extra=' result=0'),e(3,'st_channel','R9'),e(4,'st_done'),e(5,'st_channel_return','R9')]
def btf(size=240,repeat=1):
 s=b'\0request\0';t=struct.pack('<III',1,4<<24,size)*repeat
 return struct.pack('<HBBIIIII',0xeb9f,1,0,24,0,len(t),len(t),len(s))+t+s
class StageTests(unittest.TestCase):
 def test_stages_match_one_request(self):
  r,err=stages(sequence());self.assertFalse(err);a=list(r.values())[0];self.assertEqual(a['done_ns'],4);self.assertEqual(a['channel_entry_ns'],3)
 def test_pointer_reuse_is_a_new_attempt(self):
  s=sequence();s+=[dict(x,ns=x['ns']+10) for x in sequence()];r,err=stages(s);self.assertFalse(err);self.assertEqual(len(r),2)
 def test_requeue_is_not_success(self):
  r,err=stages(sequence()+[e(6,'rq_requeue')]);self.assertTrue(list(r.values())[0]['requeued'])
 def test_submission_failure_is_retained(self):
  s=sequence();s[2]=e(2,'st_queue_return',extra=' result=1055');r,err=stages(s);self.assertEqual(list(r.values())[0]['queue_result'],1055)
 def test_missing_entry_is_rejected(self):self.assertTrue(stages([e(1,'st_queue_return',extra=' result=0')])[1])
 def test_missing_return_is_rejected(self):self.assertTrue(stages(sequence()[:-1])[1])
 def test_duplicate_done_is_rejected(self):self.assertTrue(stages(sequence()+[e(6,'st_done')])[1])
 def test_other_cpu_is_not_receiving_context(self):
  s=sequence();s[4]=e(4,'st_done',cpu=2);r,_=stages(s);self.assertIsNone(list(r.values())[0]['channel_entry_ns'])
 def test_preexisting_request_is_not_invented(self):self.assertFalse(stages(sequence()[1:])[0])
 def test_completion_may_precede_queue_return(self):
  s=sequence();ret=s.pop(2);s.append(dict(ret,ns=6));r,err=stages(s);self.assertFalse(err);self.assertLess(list(r.values())[0]['done_ns'],list(r.values())[0]['queue_return_ns'])
 def test_multiple_completions_share_a_channel_callback(self):
  s=[e(0,'rq_start'),e(0,'rq_start','R2'),e(1,'st_channel','R9'),e(2,'st_done'),e(3,'st_done','R2'),e(4,'st_channel_return','R9')];r,err=stages(s);self.assertFalse(err);self.assertEqual([x['channel_entry_ns'] for x in r.values()],[1,1])
 def test_running_layout_size(self):self.assertEqual(request_size(btf()),240)
 def test_same_flush_components_sum_to_total(self):
  s=[dict(e(-3,'dummy'),body='jbd2_start_commit: dev 8,64 transaction 9'),e(-2,'fp_insert','R2')]+sequence()+[e(6,'fp_end',extra=' error=0'),e(7,'fp_seq','R2',' fq=R8 seq=4 error=0'),e(8,'fp_return')]
  r,err=join(s,0,10);self.assertFalse(err);self.assertEqual(len(r),1);a=r[0];self.assertEqual(a['total_ns'],a['pre_driver_ns']+a['driver_to_done_ns']+a['after_done_ns'])
 def test_missing_stage_rejects_flush(self):
  s=[dict(e(-3,'dummy'),body='jbd2_start_commit: dev 8,64 transaction 9'),e(-2,'fp_insert','R2')]+[x for x in sequence() if not x['body'].startswith('st_done:')]+[e(6,'fp_end',extra=' error=0'),e(7,'fp_seq','R2',' fq=R8 seq=4 error=0'),e(8,'fp_return')]
  r,err=join(s,0,10);self.assertFalse(r);self.assertIn('unmatched driver stage',err)
 def test_size_boundaries_are_inclusive(self):
  for n in [64,8192]:self.assertEqual(request_size(btf(n)),n)
 def test_ambiguous_layout_is_rejected(self):
  with self.assertRaises(ValueError):request_size(btf(repeat=2))
 def test_missing_layout_is_rejected(self):
  with self.assertRaises(ValueError):request_size(btf(repeat=0))
 def test_invalid_size_is_rejected(self):
  for n in [0,63,8193]:
   with self.assertRaises(ValueError):request_size(btf(n))
 def test_truncated_btf_is_rejected(self):
  with self.assertRaises(ValueError):request_size(btf()[:-1])
if __name__=='__main__':unittest.main()
