import copy, unittest
from analyze import phases, distribution

class Tests(unittest.TestCase):
    def sample(self):
        return {'status':'ok','diagnostic':{'commit_ok':True,'retirement':[81,82,83,1,320,84],
            'trace':[10,20,30,40,50,60,70,80], 'detail':[5,15,45,48,75,85,86,1],'finished_ns':87}}
    def test_EQ_phases_partition_post_publication_interval(self):
        x=phases(self.sample());self.assertEqual(sum(x[k] for k in list(x)[:7]),7)
    def test_BV_zero_retirements_still_have_reached_phases(self):
        x=self.sample();x['diagnostic']['retirement'][3:5]=[0,0];self.assertEqual(phases(x)['free'],1)
    def test_ER_missing_reversed_or_mismatched_records_are_rejected(self):
        for a in [None,[0]*6,[81,79,83,1,320,84],[81,82,83,129,320,84],[81,82,83,0,320,84]]:
            x=self.sample();x['diagnostic']['retirement']=a
            with self.assertRaises(AssertionError):phases(x)
        x=self.sample();del x['diagnostic']
        with self.assertRaises(KeyError):phases(x)
    def test_BV_distributions_include_empty_and_singleton(self):
        self.assertIsNone(distribution([])['max']);self.assertEqual(distribution([3])['p99'],3)

if __name__=='__main__':unittest.main()
