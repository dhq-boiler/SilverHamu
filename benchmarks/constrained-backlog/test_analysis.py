import copy,unittest
from analyze import recovery,gate
class Tests(unittest.TestCase):
    def test_ST_outstanding_ends_at_response_not_send(self):
        records=[dict(kind='write',status='ok',enqueued=i*1000000,began=10,ended=(100+i)*1000000) for i in range(8)]
        r=recovery(records,0,20000000);self.assertEqual(r['peak'],8);self.assertEqual(r['recovery_ms'],96)
        records.append(dict(kind='write',status='queue_full',enqueued=2,ended=3));self.assertEqual(recovery(records,0,20000000),r)
    def test_BV_no_backlog_cannot_be_called_an_improvement(self):
        self.assertIsNone(recovery([],0,100))
        self.assertIsNone(recovery([dict(kind='write',status='ok',enqueued=1,ended=2)],0,100))
    def test_ER_any_failed_gate_stops_extension(self):
        rows=[]
        for scenario in ['normal','single']:
            for limit in [1,8]:
                x={'scenario':scenario,'limit':limit,'mixed':{k:{'response_ms':{'p99':1,'max':2},'queue_refused':0} for k in ['read','write']},'syncs':100 if limit==1 else 80,'resources':{'rss_peak_mib':420},'recovery':{'recovery_ms':20 if limit==1 else 10}}
                rows.append(x)
        s={'trials':rows};self.assertTrue(gate(s)['proceed'])
        for field in ['recovery','read','rss','syncs','refusals','service']:
            bad=copy.deepcopy(s);normal=bad['trials'][1];single=bad['trials'][3]
            if field=='recovery':single['recovery']=None
            if field=='read':normal['mixed']['read']['response_ms']['p99']=5
            if field=='rss':normal['resources']['rss_peak_mib']=500
            if field=='syncs':single['syncs']=100
            if field=='refusals':single['mixed']['write']['queue_refused']=1
            if field=='service':single['mixed']['write']['service_refused']=1
            self.assertFalse(gate(bad)['proceed'],field)
if __name__=='__main__':unittest.main()
