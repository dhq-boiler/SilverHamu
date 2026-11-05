import importlib.util,os,unittest
from pathlib import Path
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('backlog_protocol',ROOT/'benchmarks/backlog/test_protocol.py')
base=importlib.util.module_from_spec(spec);spec.loader.exec_module(base)
class TailTests(base.BacklogTests):
    def setUp(self):
        env=patch.dict(os.environ,CATDB_DIAGNOSTICS='buffer');env.start();self.addCleanup(env.stop)
        super().setUp()
    def test_eq_delayed_batch_retains_each_result_without_inline_diagnostic(self):
        c=self.client();self.assertTrue(self.arm(c,delay_ms=100)['ok'])
        r=self.batch(c,self.entries());self.assertTrue(r['ok']);self.assertNotIn('diagnostic',r)
        d=self.ok(c,op='diagnostic_drain');self.assertEqual(d['dropped'],0)
        self.assertEqual(len(d['records']),1);row=d['records'][0]
        self.assertEqual(row['request_index'],2);self.assertEqual(len(row['items']),8)
        self.assertLessEqual(row['start_ns'],row['end_ns']);self.assertLessEqual(row['end_ns'],row['sent_ns'])
        for item,sync in zip(row['items'],row['sync_starts']):
            t=item['trace'];detail=item['detail']
            self.assertTrue(item['commit_ok']);self.assertLessEqual(t[5],sync);self.assertLessEqual(sync,t[6])
            self.assertGreaterEqual(sync-t[5],90_000_000)
            self.assertLessEqual(t[7],detail[5]);self.assertLessEqual(detail[5],detail[6])
        self.assertEqual(self.ok(c,op='diagnostic_drain')['records'],[])
    def test_st_slow_single_and_failed_request_do_not_reuse_old_trace(self):
        c=self.client();self.arm(c,delay_ms=20)
        self.ok(c,op='put_commit',key='01',value='aa')
        row=self.ok(c,op='diagnostic_drain')['records'][0]
        self.assertEqual(row['request_index'],2);self.assertEqual(len(row['items']),1)
        self.assertTrue(row['items'][0]['commit_ok'])
        self.assertFalse(self.batch(c,[])['ok'])
        self.assertEqual(self.ok(c,op='diagnostic_drain')['records'],[])
if __name__=='__main__':unittest.main()
