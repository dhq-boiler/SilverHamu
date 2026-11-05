"""Run with a lock-timing service binary and CATDB_DIAGNOSTICS=1."""
import concurrent.futures
import importlib.util
import json
import pathlib
import threading
import unittest

ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('protocol',ROOT/'tests/service_protocol.py')
protocol=importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)

class DiagnosticTests(protocol.ServiceTests):
    def check_complete(self,response):
        self.assertTrue(response['ok'],response)
        d=response['diagnostic'];t=d['trace'];p=d['detail'];c=d['costs']
        self.assertEqual((len(t),len(p),len(c)),(8,8,2))
        self.assertTrue(all(x>0 for x in t+p))
        self.assertEqual(t,sorted(t))
        self.assertLessEqual(p[7],p[0]);self.assertLessEqual(p[0],t[0])
        self.assertLessEqual(p[1],t[1])
        self.assertLessEqual(t[3],p[2]);self.assertLessEqual(p[2],p[3])
        self.assertLessEqual(p[3],t[4]);self.assertLessEqual(t[6],p[4])
        self.assertLessEqual(p[4],t[7]);self.assertLessEqual(t[7],p[5])
        self.assertLessEqual(p[5],p[6]);self.assertLessEqual(p[6],d['finished_ns'])
        self.assertLessEqual(sum(c),p[3]-p[2])
        self.assertLess(len(json.dumps(d)),2048)

    def test_diagnostic_single_mutations_and_invalid_requests_do_not_reuse_trace(self):
        client=self.client()
        self.check_complete(self.ok(client,op='put_commit',key='01',value='ab'))
        r=client.request(op='put_commit',key='not-hex',value='ab')
        self.assertFalse(r['ok']);self.assertEqual(r['diagnostic']['trace'],[0]*8)
        self.assertEqual(r['diagnostic']['detail'],[0]*8)
        self.assertEqual(r['diagnostic']['costs'],[0]*2)
        self.check_complete(self.ok(client,op='delete_commit',key='01'))
        self.ok(client,op='begin')
        self.ok(client,op='put',key='02',value='cd')
        self.check_complete(self.ok(client,op='commit'))

    def test_diagnostic_four_clients_keep_their_own_entry_and_reply_times(self):
        clients=[self.client() for _ in range(4)];barrier=threading.Barrier(4)
        def write(i):
            barrier.wait()
            return [clients[i].request(op='put_commit',key=f'{i:02x}',value=f'{n:02x}') for n in range(20)]
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            rows=list(pool.map(write,range(4)))
        for rows_per_client in rows:
            last=0
            for response in rows_per_client:
                self.check_complete(response)
                d=response['diagnostic'];self.assertGreater(d['detail'][7],last)
                last=d['finished_ns']

    def test_diagnostic_conflict_has_no_fake_sync_and_old_snapshot_is_stable(self):
        a=self.client();b=self.client()
        self.ok(a,op='put_commit',key='01',value='aa')
        self.ok(a,op='begin');self.ok(a,op='get',key='01')
        self.ok(b,op='put_commit',key='01',value='bb')
        self.assertEqual(self.ok(a,op='get',key='01')['value'],'aa')
        self.ok(a,op='put',key='02',value='cc')
        response=a.request(op='commit')
        self.assertFalse(response['ok']);self.assertEqual(response['error'],'conflict')
        d=response['diagnostic'];self.assertEqual(d['trace'][4:],[0]*4)
        self.assertGreater(d['costs'][0],0);self.assertEqual(d['costs'][1],0)
        self.assertGreater(d['detail'][6],d['detail'][7])

if __name__=='__main__':unittest.main()
