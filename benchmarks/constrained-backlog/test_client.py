import importlib.util,json,pathlib,queue,sys,tempfile,time,unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('fixtures',ROOT/'benchmarks/resource-envelope/test_client.py');fixtures=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixtures)
spec=importlib.util.spec_from_file_location('mixed',pathlib.Path(__file__).with_name('client.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Fake(fixtures.Fake):
    version=0;syncs=0;latency=0;fail=False;refuse=False
    def request(self,op,**args):
        if op=='benchmark_counters':return {'ok':True,'version':str(Fake.version),'sync_attempts':Fake.syncs,'injected_stalls':0,'injected_stall_ns':[0,0,0]}
        if op=='benchmark_stall':return {'ok':True}
        if op in ['put_commit','put_batch_commit']:
            if Fake.latency:time.sleep(Fake.latency)
            entries=args['entries'] if op=='put_batch_commit' else [args]
            with fixtures.Fake.lock:
                if Fake.refuse and len(entries)>1:return {'ok':False,'error':'resource_exhausted'}
                if Fake.fail and entries[0]['key']==fixtures.key(0).hex():raise OSError('injected disconnect')
                Fake.syncs+=1;Fake.version+=len(entries)
                for e in entries:fixtures.Fake.values[e['key']]=e['value']
            return {'ok':True,'results':[{'ok':True} for _ in entries]} if op=='put_batch_commit' else {'ok':True}
        return super().request(op,**args)
class Tests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();m.base.DATA=pathlib.Path(self.tmp.name);(m.base.DATA/'updated.json').write_text('{}');m.base.connect=Fake
        fixtures.Fake.values={fixtures.key(i).hex():fixtures.value(i).hex() for i in range(1000)};fixtures.Fake.delay=0;Fake.version=Fake.syncs=0;Fake.latency=0;Fake.fail=False;Fake.refuse=False
    def tearDown(self):self.tmp.cleanup()
    def test_EQ_ST_batched_mixed_values_and_old_generation_agree(self):
        Fake.latency=.003;r=m.measure('mixed',200,100000,64,batch_limit=8,setup_seconds=0)
        self.assertFalse(r['errors']);self.assertTrue(any(b['size']>1 for b in r['batches']));self.assertTrue(m.base.verify()['passed']);self.assertEqual(r['old_snapshot_checked_keys'],r['by_kind']['write']['completed'])
    def test_BV_outstanding_includes_active_groups_for_both_policies(self):
        for limit in [1,8]:
            Fake.latency=.01;r=m.measure('mixed',400,100000,4,batch_limit=limit,setup_seconds=0)
            self.assertGreater(r['queue_rejected'],0);self.assertFalse(r['errors'])
            for w in range(5):
                events=[]
                for e in r['records']:
                    if e['worker']==w and e['status']!='queue_full':events.extend([(e['enqueued'],1),(e['ended'],-1)])
                count=0
                for _,delta in sorted(events):count+=delta;self.assertLessEqual(count,4 if w==0 else 1)
                self.assertEqual(count,0)
    def test_ST_drain_never_waits_or_reorders_duplicate_key(self):
        q=queue.Queue();q.put((1,'a'));q.put((2,'b'));q.put(None)
        g,carry,end=m.drain(q,(0,'a'),8,lambda x:x[1]);self.assertEqual(g,[(0,'a')]);self.assertEqual(carry,(1,'a'));self.assertFalse(end)
        g,carry,end=m.drain(q,carry,8,lambda x:x[1]);self.assertEqual(g,[(1,'a'),(2,'b')]);self.assertIsNone(carry);self.assertTrue(end)
        self.assertEqual(m.drain(queue.Queue(),(0,'x'),8,lambda x:x[1]),([(0,'x')],None,False))
    def test_ER_resource_refusal_is_counted_and_successful_values_survive(self):
        Fake.refuse=True;Fake.latency=.003;r=m.measure('mixed',200,100000,64,batch_limit=8,setup_seconds=0)
        self.assertGreater(r['service_rejected'],0);self.assertFalse(r['errors'])
        self.assertEqual(r['completed']+r['queue_rejected']+r['service_rejected'],200);self.assertTrue(m.base.verify()['passed'])
    def test_ER_failed_request_drains_and_keeps_acknowledged_values(self):
        Fake.fail=True;r=m.measure('mixed',40,1000,64,batch_limit=8,setup_seconds=0)
        self.assertTrue(r['errors']);self.assertEqual(len(r['records']),40);self.assertTrue(m.base.verify()['passed'])
if __name__=='__main__':unittest.main()
