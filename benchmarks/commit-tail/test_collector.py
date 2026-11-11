import importlib.util
import pathlib
import sys
import os
import types
import unittest
from unittest import mock

ROOT=pathlib.Path(__file__).resolve().parent

class CollectorTests(unittest.TestCase):
    def load(self):
        def load_module(name, path):
            spec = importlib.util.spec_from_file_location(name, path)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            return module

        with mock.patch.dict(os.environ, BENCH_TOKEN='test', BENCH_PASSWORD='test', BENCH_OPS='2'):
            native_client = load_module('catdb_client', ROOT.parents[1]/'scripts/silverhamu-client.py')
            with mock.patch.dict(sys.modules, catdb_client=native_client):
                runner = load_module('runner', ROOT.parent/'runner.py')
        with mock.patch.dict(sys.modules, runner=runner), mock.patch.object(sys, 'path', sys.path.copy()):
            module = load_module('tail_client', ROOT/'client.py')
        module.profile=True
        obj=module.Single.__new__(module.Single);obj.request_index=7;obj.last_request_ns=100
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

    def test_shared_worker_times_the_retry_and_pulls_only_after_the_final_request(self):
        module, obj = self.load()
        obj.request_index = 0
        tick = 0
        writes = []
        probes = []

        def request(**fields):
            nonlocal tick
            if fields['op'] == 'diagnostic_last':
                probes.append(obj.request_index)
                tick += 1000
                return {'ok': True, 'diagnostic': {'request_index': obj.request_index}}
            tick += 10
            if fields['op'] == 'put_commit':
                writes.append(fields)
                if len(writes) == 1:
                    return {'ok': False, 'error': 'conflict'}
                return {'ok': True}
            self.assertEqual(fields['op'], 'get_latest')
            return {'ok': True, 'value': '00'}

        obj.client = types.SimpleNamespace(request=request, close=lambda: None)
        clock = types.SimpleNamespace(perf_counter_ns=lambda: tick)
        with mock.patch.object(module, 'time', clock), mock.patch.object(module.base, 'time', clock), \
             mock.patch.object(module.base, 'adapter', return_value=obj), \
             mock.patch.dict(os.environ, BENCH_SERIALIZATION_RETRIES='1'):
            samples, errors, changed, retries = module.base.worker(
                'catdb-service', 'update', 0, 1, 1, mock.Mock(), mock.Mock())

        self.assertEqual(samples, [20, 10])
        self.assertEqual(errors, [])
        self.assertEqual(retries, [{'worker': 0, 'sequence': 0, 'attempt': 1, 'code': 'conflict'}])
        self.assertEqual(len(writes), 3)
        self.assertEqual(writes[0], writes[1])
        self.assertTrue(changed)
        self.assertEqual(probes, [4])  # Two warmups and two attempts at the first update.
        self.assertEqual(len(module.records), 1)
        row = module.records[0]
        self.assertEqual((row['worker'], row['sequence'], row['operation_ns'], row['request_ns']),
                         (0, 0, 20, 10))
        self.assertEqual(row['probe_ns'], 1000)
        self.assertEqual(row['request_index'], row['diagnostic']['request_index'])

if __name__=='__main__':unittest.main()
