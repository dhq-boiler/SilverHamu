import importlib.util, json, pathlib, sys, tempfile, threading, time, types, unittest

class Fake:
    values={};lock=threading.Lock();delay=0
    def __init__(self):self.snapshot=None
    def close(self):pass
    def __enter__(self):return self
    def __exit__(self,*_):self.close()
    def request(self,op,**args):
        if Fake.delay:time.sleep(Fake.delay)
        with Fake.lock:
            if op=='begin_read':self.snapshot=dict(Fake.values)
            if op=='put_commit':Fake.values[args['key']]=args['value']
            if op in ['get','get_latest']:return {'ok':True,'value':(self.snapshot if self.snapshot is not None else Fake.values).get(args['key'])}
            if op=='scan':
                src=self.snapshot if self.snapshot is not None else Fake.values
                return {'ok':True,'rows':[(k,v) for k,v in sorted(src.items()) if k>=args['start']][:args.get('rows',100)]}
        return {'ok':True}

sys.modules['catdb_client']=types.SimpleNamespace(Client=None)
key=lambda i:i.to_bytes(8,'big')
value=lambda i,stamp=0:key(i)+key(stamp)+b'x'*240
sys.modules['runner']=types.SimpleNamespace(key=key,value=value,ROWS=1000,TOKEN='test')
spec=importlib.util.spec_from_file_location('resource_client',pathlib.Path(__file__).with_name('client.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class WorkloadTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();m.DATA=pathlib.Path(self.tmp.name);(m.DATA/'updated.json').write_text('{}');m.connect=Fake
        Fake.values={key(i).hex():value(i).hex() for i in range(1000)};Fake.delay=0
    def tearDown(self):self.tmp.cleanup()
    def test_EQ_ST_mixed_then_reads_preserve_acknowledged_values_and_old_snapshot(self):
        r=m.measure('mixed',100,5000,64);self.assertFalse(r['errors']);self.assertEqual(r['old_snapshot_checked_keys'],20);self.assertTrue(m.verify()['passed'])
        r=m.measure('read',100,5000,64);self.assertFalse(r['errors']);self.assertTrue(m.verify()['passed'])
    def test_EQ_ST_unpinned_workload_still_verifies_updates_and_rejects_bad_values(self):
        r=m.measure('mixed',100,5000,64,pin_reader=False)
        self.assertFalse(r['pin_reader']);self.assertEqual(r['old_snapshot_checked_keys'],0)
        self.assertFalse(r['errors']);self.assertTrue(m.verify()['passed'])
        Fake.values[key(0).hex()]='00'
        with self.assertRaises(AssertionError):m.verify()
    def test_BV_ER_bounded_queue_rejects_and_drains_without_stranding(self):
        Fake.delay=.002;r=m.measure('mixed',200,100000,1)
        self.assertGreater(r['queue_rejected'],0);self.assertFalse(r['errors']);self.assertEqual(r['completed']+r['queue_rejected'],200)
        self.assertTrue(all(x['planned']<=x['enqueued']<=x['ended'] for x in r['records']))
    def test_ER_corrupted_values_fail_full_verification(self):
        Fake.values[key(99).hex()]='00'
        with self.assertRaises(AssertionError):m.verify()
    def test_ST_observer_gate_finishes_once_before_timed_arrivals(self):
        ready=[]
        r=m.measure('mixed',20,1000,64,before_start=lambda:ready.append(time.monotonic_ns()))
        self.assertEqual(len(ready),1);self.assertLessEqual(ready[0],r['start_ns'])
        self.assertTrue(all(x['planned']>=ready[0] for x in r['records']))
        self.assertFalse(r['errors']);self.assertTrue(m.verify()['passed'])
    def test_BV_invalid_capacity_rejected(self):
        with self.assertRaises(AssertionError):m.measure('read',10,100,0)

    def test_ST_ER_safety_probe_preserves_old_and_staged_values_after_refusal(self):
        class Staged(Fake):
            def __init__(self):super().__init__();self.pending=None
            def request(self,op,**args):
                if op=='begin':self.pending={};return {'ok':True}
                if op=='put':
                    if len(bytes.fromhex(args['value']))>4096:return {'ok':False,'error':'resource_exhausted'}
                    self.pending[args['key']]=args['value'];return {'ok':True}
                if op=='get' and self.pending is not None and args['key'] in self.pending:return {'ok':True,'value':self.pending[args['key']]}
                if op=='rollback':self.pending=None;self.snapshot=None;return {'ok':True}
                return super().request(op,**args)
        m.connect=Staged
        result=m.safety_probe();self.assertEqual(result['old_generation_updates_checked'],8)
        self.assertTrue(result['prior_staged_value_preserved']);self.assertTrue(m.verify()['passed'])
    def test_ER_safety_probe_rejects_a_false_resource_failure(self):
        with self.assertRaises(AssertionError):m.safety_probe()

    def test_BV_percentiles_include_empty_and_single_sample(self):
        self.assertIsNone(m.quantile([],.99));self.assertEqual(m.quantile([2000000],.99),2)

if __name__=='__main__':unittest.main()
