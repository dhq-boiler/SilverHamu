import copy
import unittest
from report import validate_events, cadence, attribute

class ReportTests(unittest.TestCase):
    def fixture(self):
        run = dict(attempted=2, clients=1, rate=10, completed=1, rejected=1, errors=[], sync_attempts=1,
                   schedule=dict(origin_ns=1, queue_capacity=1, queue_high_water=[1],
                                 counters=dict(before=dict(version='4', sync_attempts=2), after=dict(version='5', sync_attempts=3))))
        events = [dict(id=0, worker=0, sequence=0, planned_ns=1, admitted_ns=2, sent_ns=3, finished_ns=4, status='completed'),
                  dict(id=1, worker=0, sequence=1, planned_ns=100_000_001, admitted_ns=100_000_002, finished_ns=100_000_002, status='rejected')]
        return run, events

    def test_rejections_and_delayed_sends_remain_visible(self):
        r, e = self.fixture()
        validate_events(r, e)
        bins = cadence(r, e, 100_000_000)
        self.assertEqual([b['planned'] for b in bins], [1, 1])
        self.assertEqual([b['sent'] for b in bins], [1, 0])

    def test_missing_reordered_or_shifted_arrivals_cannot_pass(self):
        r, e = self.fixture()
        for bad in [e[:1], list(reversed(e))]:
            with self.assertRaises(AssertionError): validate_events(r, bad)
        for field in ['planned_ns', 'worker', 'sequence']:
            bad = copy.deepcopy(e)
            bad[0][field] += 1
            with self.assertRaises(AssertionError): validate_events(r, bad)

    def test_counter_delta_and_capacity_mismatches_cannot_pass(self):
        r, e = self.fixture()
        for field in ['version', 'sync_attempts']:
            bad = copy.deepcopy(r)
            bad['schedule']['counters']['after'][field] = 99
            with self.assertRaises(AssertionError): validate_events(bad, e)
        r['schedule']['queue_high_water'] = [2]
        with self.assertRaises(AssertionError): validate_events(r, e)

    def attribution_fixture(self):
        diagnostic = dict(request_index=51, commit_ok=True, trace=[4,6,7,8,12,13,14,16],
                          detail=[3,5,9,11,15,17,18,2], costs=[1,1], dispatch_ns=18, finished_ns=19)
        row = dict(id=0, ok=True, request_index=51, operation_ns=22, response_ns=32,
                   request_ns=20, worker=0, sequence=0, diagnostic=diagnostic)
        event = dict(id=0, worker=0, sequence=0, status='completed', planned_ns=1,
                     admitted_ns=4, sent_ns=11, finished_ns=33)
        return dict(attempted=1,clients=1), [event], dict(limit=1,dropped=0,records=[row])

    def test_queue_and_operation_phases_sum_to_the_same_response(self):
        r, e, raw = self.attribution_fixture()
        phases = attribute(r, e, raw)[0]['phases']
        self.assertEqual(sum(phases.values()), 32)
        self.assertEqual(phases['producer scheduling delay'], 3)
        self.assertEqual(phases['client queue'], 7)

    def test_wrong_request_duration_and_missing_records_are_rejected(self):
        for case in ['request', 'duration', 'missing']:
            r, e, raw = self.attribution_fixture()
            if case == 'request': raw['records'][0]['request_index'] += 1
            elif case == 'duration': raw['records'][0]['response_ns'] += 1
            else: raw['records'] = []
            with self.assertRaises(AssertionError): attribute(r, e, raw)

if __name__ == '__main__': unittest.main()
