import unittest
from analyze import FULL,quality,profile
def meta(mode='full'):
 names=FULL if mode in ('full','capacity') else FULL-{'rq_end','rq_update','rq_update_return'} if mode=='minimal' else set()
 return dict(mode=mode,request_probe_count=len(names),probe_profile=[n+' 100 0' for n in sorted(names)],profile_before=[n+' 0 0' for n in sorted(names)],cleaned=True,stats={'cpu0':'overrun: 0\ncommit overrun: 0\ndropped events: 0'})
class QualityTests(unittest.TestCase):
 def test_all_modes_require_exact_observer_sets(self):
  for mode in ['full','minimal','capacity','standard']:self.assertFalse(quality(meta(mode)))
 def test_missing_probe_is_rejected(self):
  m=meta();m['probe_profile'].pop();self.assertIn('probe set mismatch',quality(m))
 def test_entry_miss_is_rejected(self):
  m=meta();m['probe_profile']=[s.replace('100 0','100 1') if s.startswith('st_done ') else s for s in m['probe_profile']];self.assertIn('missed probes',quality(m))
 def test_return_only_miss_is_rejected(self):
  m=meta('capacity');m['probe_profile']=[s.replace('100 0','100 1') if s.startswith('fp_return ') else s for s in m['probe_profile']];self.assertIn('missed probes',quality(m))
 def test_startup_miss_is_not_waived(self):
  m=meta();m['profile_before'][0]=m['profile_before'][0].replace('0 0','0 1');m['probe_profile'][0]=m['probe_profile'][0].replace('100 0','100 1');self.assertIn('missed probes',quality(m))
 def test_counter_regression_is_rejected(self):
  m=meta();m['profile_before'][0]=m['profile_before'][0].replace('0 0','101 0');self.assertIn('counter regression',quality(m))
 def test_duplicate_profile_is_rejected(self):
  with self.assertRaises(ValueError):profile(['x 1 0','x 1 0'])
 def test_unknown_mode_is_rejected(self):
  m=meta();m['mode']='unknown';self.assertTrue(quality(m))
 def test_ring_loss_and_missing_counters_are_rejected(self):
  for value in ['overrun: 1\ncommit overrun: 0\ndropped events: 0','overrun: 0','']:
   m=meta();m['stats']['cpu0']=value;self.assertTrue(quality(m))
 def test_cleanup_failure_is_rejected(self):
  m=meta();m['cleaned']=False;self.assertTrue(quality(m))
 def test_negative_counter_is_rejected(self):
  with self.assertRaises(ValueError):profile(['x -1 0'])
if __name__=='__main__':unittest.main()
