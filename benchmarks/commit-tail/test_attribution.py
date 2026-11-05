import importlib.util
import pathlib
import unittest

ROOT=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('tail_report',ROOT/'report.py')
report=importlib.util.module_from_spec(spec);spec.loader.exec_module(report)

class AttributionTests(unittest.TestCase):
    def fixture(self):
        run={'completed':2,'attempted':2,'clients':1,'errors':[]}
        diagnostic={'request_index':51,'commit_ok':True,'trace':[4,6,7,8,12,13,14,16],
            'detail':[3,5,9,11,15,17,18,2],'costs':[1,1],'dispatch_ns':18,'finished_ns':19}
        row={'ok':True,'request_index':51,'operation_ns':22,'request_ns':20,'worker':0,'sequence':0,'diagnostic':diagnostic}
        return run,[22,21],{'dropped':0,'limit':2,'records':[row]}

    def test_exact_operation_correspondence_and_phase_sum(self):
        run,latency,raw=self.fixture()
        result=report.validate_run(run,latency,raw)
        self.assertEqual(sum(result[0]['phases'].values()),22)

    def test_wrong_request_wrong_latency_missing_maximum_and_overflow_are_rejected(self):
        for case in ['request','latency','maximum','overflow']:
            run,latency,raw=self.fixture()
            if case=='request':raw['records'][0]['request_index']=52
            elif case=='latency':raw['records'][0]['operation_ns']=23
            elif case=='maximum':latency[1]=23
            else:raw['dropped']=1
            with self.assertRaises(AssertionError):report.validate_run(run,latency,raw)

    def test_missing_threshold_exceedance_cannot_be_hidden_by_recording_only_maximum(self):
        run,latency,raw=self.fixture()
        raw['records'][0]['operation_ns']=30_000_000
        latency=[30_000_000,20_000_000]
        with self.assertRaises(AssertionError):report.validate_run(run,latency,raw)

if __name__=='__main__':unittest.main()
