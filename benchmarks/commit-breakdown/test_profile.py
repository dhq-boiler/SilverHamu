import importlib.util
import pathlib
import sys
import types
import unittest

ROOT=pathlib.Path(__file__).resolve().parent

class ProfileTests(unittest.TestCase):
    def load_report(self):
        spec=importlib.util.spec_from_file_location('breakdown_report',ROOT/'report.py')
        module=importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def sample(self):
        return {'ok':True,'request_ns':20,'diagnostic':{'trace':[4,6,7,8,12,13,14,16],
            'detail':[3,5,9,11,15,17,18,2],'costs':[1,1],'dispatch_ns':18,'finished_ns':19}}

    def test_phase_sum_matches_request_and_handles_arrival_during_collection(self):
        phases=self.load_report().phases
        row=self.sample()
        self.assertEqual(sum(phases(row).values()),20)
        row['diagnostic']['trace'][0]=6
        result=phases(row)
        self.assertEqual(result['leader queue'],0)
        self.assertEqual(result['batch collection'],0)
        self.assertEqual(sum(result.values()),20)

    def test_missing_reversed_and_double_counted_diagnostics_are_rejected(self):
        phases=self.load_report().phases
        for mutation in ['missing','reversed','cost']:
            row=self.sample()
            if mutation=='missing':row['diagnostic']['trace'][6]=0
            elif mutation=='reversed':row['diagnostic']['detail'][4]=17
            else:row['diagnostic']['costs']=[2,2]
            with self.assertRaises(AssertionError):phases(row)

    def load_client(self):
        stub=types.ModuleType('runner')
        stub.Remote=type('Remote',(),{})
        stub.OPS=2
        previous=sys.modules.get('runner')
        sys.modules['runner']=stub
        try:
            spec=importlib.util.spec_from_file_location('profile_client',ROOT/'client.py')
            module=importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
        finally:
            if previous is None:sys.modules.pop('runner',None)
            else:sys.modules['runner']=previous
        module.profile=True
        return module

    def test_profile_limit_records_boundary_and_counts_excess_without_affecting_success(self):
        module=self.load_client()
        client=module.Single()
        client.client=types.SimpleNamespace(request=lambda **kw:{'ok':True,'diagnostic':{'trace':[]}})
        for n in range(4):
            self.assertTrue(client.request(op='put_commit')['ok'])
            self.assertEqual(len(module.records),min(n+1,2))
            self.assertEqual(module.dropped,max(0,n-1))

    def test_failed_response_is_recorded_before_reporting_error(self):
        module=self.load_client()
        client=module.Single()
        client.client=types.SimpleNamespace(request=lambda **kw:{'ok':False,'error':'conflict','diagnostic':{}})
        with self.assertRaises(RuntimeError):client.request(op='commit')
        self.assertEqual(len(module.records),1)
        self.assertFalse(module.records[0]['ok'])
        self.assertEqual(module.records[0]['error'],'conflict')

if __name__=='__main__':unittest.main()
