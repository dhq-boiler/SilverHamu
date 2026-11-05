"""Quiescent benchmark counters must not mutate data or transaction state."""
import importlib.util
import os
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('protocol', ROOT / 'tests/service_protocol.py')
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)

class CounterTests(protocol.ServiceTests):
    def test_counters_are_optional_and_do_not_publish_pending_writes(self):
        a = self.client()
        first = a.request(op='benchmark_counters')
        if os.environ.get('CATDB_EXPECT_METRICS') != '1':
            self.assertFalse(first['ok'])
            self.assertEqual(self.ok(a, op='ping')['version'], '0')
            return
        self.assertTrue(first['ok'])
        self.assertEqual(first, {'ok': True, 'version': '0', 'sync_attempts': 0})
        self.ok(a, op='begin')
        self.ok(a, op='put', key='01', value='aa')
        self.assertEqual(self.ok(a, op='benchmark_counters'), first)
        b = self.client()
        self.assertIsNone(self.ok(b, op='get_latest', key='01')['value'])
        self.ok(a, op='commit')
        committed = self.ok(a, op='benchmark_counters')
        self.assertEqual(committed['sync_attempts'], 1)
        self.assertEqual(committed['version'], '1')
        self.assertEqual(self.ok(a, op='benchmark_counters'), committed)
        self.ok(a, op='begin_read')
        self.ok(b, op='put_commit', key='01', value='bb')
        self.assertEqual(self.ok(a, op='benchmark_counters')['sync_attempts'], 2)
        self.assertEqual(self.ok(a, op='get', key='01')['value'], 'aa')
        self.ok(a, op='commit')
        self.assertFalse(b.request(op='put_commit', key='not-hex', value='cc')['ok'])
        self.assertEqual(self.ok(a, op='benchmark_counters')['sync_attempts'], 2)
        self.stop()
        self.start()
        c = self.client()
        self.assertEqual(self.ok(c, op='get_latest', key='01')['value'], 'bb')
        self.assertEqual(self.ok(c, op='benchmark_counters')['sync_attempts'], 0)

if __name__ == '__main__':
    unittest.main()
