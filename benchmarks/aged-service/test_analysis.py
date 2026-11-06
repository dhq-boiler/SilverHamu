import unittest
from analyze import measured,stats

class Tests(unittest.TestCase):
    def sample(self,t,cpu,rss):
        return {'time_ns':t,'rss':rss,'memory':3*2**20,'swap':0,'stat':{'file':2**20,'anon':2**20},'cpu':{'usage_usec':cpu,'throttled_usec':0}}
    def test_EQ_sampling_uses_only_workload_interval_and_separates_cache(self):
        a=[self.sample(0,0,20*2**20),self.sample(1000000000,100,2**20),self.sample(2000000000,500100,2*2**20)]
        x=measured(a,1000000000,2000000000);self.assertEqual(x['rss_peak_mib'],2);self.assertEqual(x['cpu_core_percent'],50);self.assertEqual(x['file_cache_peak_mib'],1)
    def test_ER_insufficient_samples_are_not_reported_as_zero(self):
        with self.assertRaises(AssertionError):measured([self.sample(0,0,1)],0,1)
    def test_BV_percentiles_and_empty(self):
        self.assertIsNone(stats([])['p99']);self.assertEqual(stats([5])['p99'],5);self.assertEqual(stats(list(range(100)))['p99'],98)

if __name__=='__main__':unittest.main()
