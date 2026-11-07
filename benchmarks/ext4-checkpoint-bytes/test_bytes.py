import unittest
from join import byte_bins

class ByteWindows(unittest.TestCase):
    def test_half_open_window_and_mixed_targets(self):
        events=[(0,16,3,3),(1,32,4,4),(2,64,3,4),(3,128,0,4),(4,256,4,None),(5,512,9,4),(6,1024,3,3)]
        times=[e[0] for e in events]
        self.assertEqual(byte_bins(events,times,0,6,3),(16,544,448))
        self.assertEqual(byte_bins(events,times,6,7,3),(1024,0,0))
        self.assertEqual(byte_bins(events,times,2,2,3),(0,0,0))

    def test_bytes_at_equal_timestamps_are_counted_once(self):
        events=[(5,32,4,9),(5,64,5,4)]
        self.assertEqual(byte_bins(events,[5,5],4,5,3),(0,0,0))
        self.assertEqual(byte_bins(events,[5,5],5,6,3),(0,96,0))

class ExposureControls(unittest.TestCase):
    def test_volume_boundary_and_unknown_bytes(self):
        from report_bytes import exposure
        row=dict(prior_same_B=0,prior_other_B=2**20-1,prior_unknown_B=0)
        self.assertEqual(exposure(row,"prior"),"other <1 MiB")
        row["prior_other_B"]+=1
        self.assertEqual(exposure(row,"prior"),"other >=1 MiB")
        row["prior_unknown_B"]=1
        self.assertEqual(exposure(row,"prior"),"mixed/same/unknown")

    def test_control_must_share_publication_and_phase_and_be_nearby(self):
        from report_bytes import local_controls
        def row(t,pub,phase,other):
            return dict(start_ns=t,tx=t,publication=pub,phase=phase,duration_us=10,
                        prior_other_B=other,prior_same_B=0,prior_unknown_B=0)
        exposed=row(1_000_000_001,7,"snapshot",1024)
        controls=[row(0,7,"snapshot",0),row(1_000_000_002,8,"snapshot",0),row(1_000_000_002,7,"encode",0)]
        self.assertEqual(local_controls([exposed]+controls),[])
        self.assertEqual(local_controls([exposed]+controls+[row(1,7,"snapshot",0)])[0]["control_start_ns"],1)

if __name__=="__main__": unittest.main()
