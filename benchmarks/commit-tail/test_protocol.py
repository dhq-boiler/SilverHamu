"""Run with CATDB_DIAGNOSTICS=pull and a lock-timing service binary."""
import concurrent.futures
import importlib.util
import pathlib
import os
from unittest.mock import patch
import unittest

ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('protocol',ROOT/'tests/service_protocol.py')
protocol=importlib.util.module_from_spec(spec);spec.loader.exec_module(protocol)

class PullTests(protocol.ServiceTests):
    def pull(self,c):return self.ok(c,op='diagnostic_last')['diagnostic']

    def test_pull_command_requires_explicit_opt_in(self):
        self.stop()
        with patch.dict(os.environ):
            os.environ.pop('CATDB_DIAGNOSTICS',None)
            self.start()
        c=self.client()
        response=c.request(op='diagnostic_last')
        self.assertFalse(response['ok'])
        self.assertNotIn('diagnostic',response)

    def test_pull_is_bounded_to_previous_request_and_does_not_change_normal_response(self):
        c=self.client()
        self.assertIsNone(self.pull(c))
        r=self.ok(c,op='put_commit',key='01',value='aa')
        self.assertNotIn('diagnostic',r)
        d=self.pull(c);self.assertEqual(d['request_index'],1);self.assertTrue(d['commit_ok'])
        self.assertEqual(d,self.pull(c))
        self.assertEqual([len(d[k]) for k in ['trace','detail','costs']],[8,8,2])
        self.ok(c,op='ping');self.assertIsNone(self.pull(c))
        self.ok(c,op='delete_commit',key='01')
        self.assertEqual(self.pull(c)['request_index'],3)

    def test_failed_commit_replaces_the_old_trace_instead_of_reusing_it(self):
        c=self.client();self.ok(c,op='put_commit',key='01',value='aa')
        old=self.pull(c)
        r=c.request(op='put_commit',key='not-hex',value='aa');self.assertFalse(r['ok'])
        d=self.pull(c);self.assertFalse(d['commit_ok']);self.assertEqual(d['request_index'],2)
        self.assertEqual(d['trace'],[0]*8);self.assertGreater(d['finished_ns'],old['finished_ns'])
        self.assertEqual(self.ok(c,op='get_latest',key='01')['value'],'aa')

    def test_connections_do_not_share_diagnostics_and_survive_one_four_one_transitions(self):
        a=self.client();self.ok(a,op='put_commit',key='01',value='aa');old=self.pull(a)
        others=[self.client() for _ in range(3)]
        for c in others:self.assertIsNone(self.pull(c))
        def put(pair):
            i,c=pair
            self.ok(c,op='put_commit',key=f'{i+2:02x}',value='bb')
            return self.pull(c)
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            ds=list(pool.map(put,enumerate(others)))
        self.assertEqual(old,self.pull(a))
        self.assertTrue(all(d['request_index']==1 and d['commit_ok'] for d in ds))
        for c in others:c.close();self.clients.remove(c)
        self.ok(a,op='put_commit',key='01',value='cc')
        self.assertEqual(self.pull(a)['request_index'],2)
        for i in range(2,5):self.assertEqual(self.ok(a,op='get_latest',key=f'{i:02x}')['value'],'bb')

    def test_conflict_record_does_not_report_a_sync_for_the_rejected_update(self):
        a=self.client();b=self.client()
        self.ok(a,op='put_commit',key='01',value='aa')
        self.ok(a,op='begin');self.ok(a,op='get',key='01')
        self.ok(b,op='put_commit',key='01',value='bb')
        self.assertEqual(self.ok(a,op='get',key='01')['value'],'aa')
        self.ok(a,op='put',key='02',value='cc')
        response=a.request(op='commit');self.assertFalse(response['ok'])
        d=self.pull(a);self.assertFalse(d['commit_ok']);self.assertEqual(d['trace'][4:],[0]*4)

if __name__=='__main__':unittest.main()
