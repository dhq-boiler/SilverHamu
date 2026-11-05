"""Independent batch outcomes preserve snapshots, boundedness and durability."""
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import struct
import time
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('protocol',ROOT/'tests/service_protocol.py')
protocol=importlib.util.module_from_spec(spec);spec.loader.exec_module(protocol)

class BacklogTests(protocol.ServiceTests):
    def setUp(self):
        self.env=patch.dict(os.environ,CATDB_BENCH_STALL='1');self.env.start()
        self.addCleanup(self.env.stop)
        super().setUp()
    def entries(self,n=8):return [dict(key=f'{i+1:02x}',value=f'{i:02x}') for i in range(n)]
    def batch(self,c,entries):return c.request(op='put_batch_commit',entries=entries)
    def counters(self,c):return self.ok(c,op='benchmark_counters')
    def arm(self,c,**kw):
        return c.request(op='benchmark_stall',**(dict(start_ms=0,duration_ms=10000,delay_ms=20,count=1)|kw))
    def test_eq_each_update_has_a_version_one_sync_and_survives_process_exit(self):
        c=self.client();before=self.counters(c)
        r=self.batch(c,self.entries());self.assertTrue(r['ok']);self.assertFalse(r['reopen_required'])
        self.assertTrue(all(x['ok'] for x in r['results']))
        self.assertEqual([int(x['version']) for x in r['results']],list(range(1,9)))
        self.assertEqual(self.counters(c)['sync_attempts']-before['sync_attempts'],1)
        self.stop();self.start();c=self.client()
        self.assertEqual(self.ok(c,op='scan_latest',start='')['rows'],[[x['key'],x['value']] for x in self.entries()])
    def test_bv_bad_count_size_and_duplicate_key_reject_before_any_update(self):
        c=self.client()
        invalid=[[],self.entries(9),[dict(key='01',value='aa'),dict(key='01',value='bb')],
                 [dict(key='01',value='aa'),dict(key='zz',value='bb')],
                 [dict(key='01',value='aa'*(256*1024))]]
        for entries in invalid:
            self.assertFalse(self.batch(c,entries)['ok'])
            self.assertEqual(self.counters(c)['version'],'0')
        self.assertTrue(self.batch(c,[dict(key='01',value='aa'*(256*1024-1))])['results'][0]['ok'])
    def test_st_existing_transaction_is_preserved_and_old_read_observes_old_generation(self):
        c=self.client();self.ok(c,op='begin');self.ok(c,op='put',key='01',value='ff')
        self.assertFalse(self.batch(c,self.entries())['ok'])
        self.assertEqual(self.ok(c,op='get',key='01')['value'],'ff');self.ok(c,op='commit')
        self.ok(c,op='begin');self.ok(c,op='scan',start='')
        b=self.client();self.assertTrue(self.batch(b,self.entries())['ok'])
        self.assertEqual(self.ok(c,op='get',key='01')['value'],'ff')
        self.ok(c,op='put',key='ff',value='ff')
        self.assertEqual(c.request(op='commit')['error'],'conflict')
    def test_st_same_key_in_successive_batches_retains_request_order(self):
        c=self.client()
        for value in ['aa','bb','cc']:
            self.assertTrue(self.batch(c,[dict(key='01',value=value)])['results'][0]['ok'])
        self.assertEqual(self.ok(c,op='get_latest',key='01')['value'],'cc')
    def test_bv_transaction_budget_rejection_releases_all_staged_transactions(self):
        self.stop();self.start('--max-connections','1');c=self.client()
        self.assertFalse(self.batch(c,self.entries(2))['ok'])
        self.assertEqual(self.counters(c)['version'],'0')
        self.ok(c,op='put_commit',key='ff',value='aa')
    def test_st_four_connections_commit_disjoint_batches_without_lost_outcomes(self):
        clients=[self.client() for _ in range(4)]
        def write(pair):
            i,c=pair
            return self.batch(c,[dict(key=f'{i:02x}{n:02x}',value='aa') for n in range(8)])
        with concurrent.futures.ThreadPoolExecutor(4) as pool:results=list(pool.map(write,enumerate(clients)))
        self.assertTrue(all(r['ok'] and all(x['ok'] for x in r['results']) for r in results))
        self.assertEqual(len(self.ok(clients[0],op='scan_latest',start='')['rows']),32)
    def test_st_conflicting_entry_does_not_hide_the_other_independent_commit(self):
        a=self.client();b=self.client()
        self.assertTrue(self.arm(a,delay_ms=1000)['ok'])
        with concurrent.futures.ThreadPoolExecutor(1) as pool:
            first=pool.submit(self.batch,a,[dict(key='01',value='aa'),dict(key='02',value='bb')])
            deadline=time.monotonic()+3
            while self.counters(b)['injected_stalls']==0 and time.monotonic()<deadline:time.sleep(.001)
            self.assertFalse(first.done())
            result=self.batch(b,[dict(key='01',value='cc'),dict(key='03',value='dd')])
            self.assertTrue(first.result()['ok'])
        self.assertTrue(result['ok'])
        self.assertEqual(result['results'][0]['error'],'conflict')
        self.assertTrue(result['results'][1]['ok'])
        self.assertEqual(self.ok(b,op='scan_latest',start='')['rows'],[['01','aa'],['02','bb'],['03','dd']])

    def test_er_disconnect_does_not_make_a_submitted_batch_uncommitted_by_assumption(self):
        c=self.client();self.arm(c,delay_ms=100)
        body=json.dumps(dict(op='put_batch_commit',entries=self.entries())).encode()
        c.socket.sendall(struct.pack('!I',len(body))+body);c.close();self.clients.remove(c)
        b=self.client();deadline=time.monotonic()+3
        while self.counters(b)['version']!='8' and time.monotonic()<deadline:time.sleep(.01)
        self.assertEqual(self.counters(b)['version'],'8')
        self.stop();self.start();b=self.client()
        self.assertEqual(len(self.ok(b,op='scan_latest',start='')['rows']),8)
    def test_bv_stall_count_zero_one_and_invalid_limits_do_not_mutate_data(self):
        c=self.client()
        for kw in [dict(count=-1),dict(delay_ms=1001),dict(duration_ms=10001),dict(start_ms=-1)]:
            self.assertFalse(self.arm(c,**kw)['ok'])
        self.assertEqual(self.counters(c)['version'],'0')
        self.assertTrue(self.arm(c,count=0)['ok'])
        self.ok(c,op='put_commit',key='01',value='aa')
        self.assertEqual(self.counters(c)['injected_stalls'],0)
        self.assertTrue(self.arm(c)['ok'])
        for _ in range(3):self.ok(c,op='put_commit',key='01',value='bb')
        counters=self.counters(c)
        self.assertEqual(counters['injected_stalls'],1)
        first,last,elapsed=counters['injected_stall_ns']
        self.assertGreater(first,0);self.assertGreaterEqual(elapsed,20_000_000)
        self.assertEqual(last-first,elapsed)
    def test_er_stall_requires_explicit_runtime_opt_in(self):
        self.stop()
        with patch.dict(os.environ):
            os.environ.pop('CATDB_BENCH_STALL',None);self.start()
        c=self.client();self.assertFalse(self.arm(c)['ok'])
        self.ok(c,op='put_commit',key='01',value='aa')

if __name__=='__main__':unittest.main()
