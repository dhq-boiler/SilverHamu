import importlib.util
import pathlib
import sys
import time
import types
import unittest

ROOT=pathlib.Path(__file__).resolve().parent

class CollectorTests(unittest.TestCase):
    def load(self):
        stub=types.ModuleType('runner');stub.Remote=type('Remote',(),{});stub.OPS=2
        for name in ['adapter','random','ROWS','key','value']:setattr(stub,name,None)
        stub.time=time
        old=sys.modules.get('runner');sys.modules['runner']=stub
        sys.path.insert(0,str(ROOT))
        try:
            spec=importlib.util.spec_from_file_location('tail_client',ROOT/'client.py')
            module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        finally:
            sys.path.remove(str(ROOT))
            if old is None:sys.modules.pop('runner',None)
            else:sys.modules['runner']=old
        module.profile=True
        obj=module.Single();obj.request_index=7;obj.last_request_ns=100
        obj.client=types.SimpleNamespace(request=lambda **kw:{'ok':True,'diagnostic':{'request_index':7}})
        return module,obj

    def test_threshold_before_at_after_and_record_count_limit(self):
        module,obj=self.load();obj.maximum_ns=100_000_000
        for x in [9_999_999,10_000_000,10_000_001]:obj.after_sample(x,0,0)
        self.assertEqual([r['operation_ns'] for r in module.records],[10_000_000,10_000_001])
        obj.after_sample(20_000_000,0,1)
        self.assertEqual(len(module.records),2);self.assertEqual(module.dropped,1)

    def test_new_maxima_below_threshold_are_saved_and_correspondence_failure_is_explicit(self):
        module,obj=self.load()
        for x in [3,2,4]:obj.after_sample(x,0,0)
        self.assertEqual([r['operation_ns'] for r in module.records],[3,4])
        module.records.clear()
        obj.client=types.SimpleNamespace(request=lambda **kw:{'ok':True,'diagnostic':{'request_index':6}})
        obj.after_sample(10_000_001,0,1)
        self.assertIn('correspondence mismatch',module.records[0]['probe_error'])

    def test_disabled_collection_does_no_network_work(self):
        module,obj=self.load();module.profile=False
        obj.client=types.SimpleNamespace(request=lambda **kw: self.fail('unexpected probe'))
        obj.after_sample(50_000_000,0,0)
        self.assertEqual(module.records,[])

    def test_workload_and_timer_match_original_except_post_sample_hook(self):
        original=(ROOT.parent/'runner.py').read_text(encoding='utf-8')
        original=original[original.index('def worker('):original.index('\ndef measure(')].strip()
        wanted=original.replace('            samples.append(time.perf_counter_ns()-began)',
            '            elapsed=time.perf_counter_ns()-began\n            samples.append(elapsed)\n            a.after_sample(elapsed,worker_id,sequence)')
        actual=(ROOT/'worker.py').read_text(encoding='utf-8')
        actual=actual[actual.index('def worker('):].strip()
        self.assertEqual(wanted,actual)

if __name__=='__main__':unittest.main()
