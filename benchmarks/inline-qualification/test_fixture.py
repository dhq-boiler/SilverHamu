import importlib.util, os, pathlib, sys, unittest
ROOT=pathlib.Path(__file__).resolve().parents[2]
os.environ.update(BENCH_PASSWORD='fixture-only',BENCH_TOKEN='fixture-only',BENCH_KEY_SHAPE='mixed')
spec=importlib.util.spec_from_file_location('catdb_client',ROOT/'scripts/catdb-client.py')
client=importlib.util.module_from_spec(spec);spec.loader.exec_module(client);sys.modules['catdb_client']=client
spec=importlib.util.spec_from_file_location('fixture',ROOT/'benchmarks/runner.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
class FixtureTests(unittest.TestCase):
    def test_EQ_half_short_half_long_and_order_preserved(self):
        keys=[fixture.key(i) for i in range(1000000)]
        self.assertEqual(sum(len(k)==8 for k in keys),500000)
        self.assertEqual(sum(len(k)==32 for k in keys),500000)
        self.assertTrue(all(a<b for a,b in zip(keys,keys[1:])))
    def test_BV_values_remain_256_bytes_and_end_is_outside(self):
        for i in (0,1,14,15,16,255,256,999999,1000000):
            self.assertEqual(len(fixture.value(i,3)),256)
            self.assertEqual(fixture.value(i,3)[:8],i.to_bytes(8,'big'))
        self.assertLess(fixture.key(999999),fixture.key(1000000))
if __name__=='__main__':unittest.main()
