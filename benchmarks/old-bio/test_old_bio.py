import unittest

from analyze_bio import classify, WINDOW

class OutstandingTests(unittest.TestCase):
    def test_exactly_ten_ms_and_completion_at_sync_start(self):
        start=2*WINDOW
        pending=[(WINDOW,start+1,4096),(WINDOW-1,start,8192)]
        self.assertEqual(classify(start,start+10,pending,[WINDOW-1,WINDOW]),(1,0,1,4096))

    def test_quiet_sync_can_have_two_older_writes_in_flight(self):
        pending=[(1,3*WINDOW,4096),(2,4*WINDOW,8192)]
        self.assertEqual(classify(2*WINDOW,3*WINDOW,pending,[1,2,3*WINDOW]),(0,0,2,12288))

    def test_submission_at_sync_start_is_in_during_window(self):
        self.assertEqual(classify(WINDOW,WINDOW+10,[],[WINDOW,WINDOW+10]),(0,1,0,0))

if __name__=="__main__":
    unittest.main()
