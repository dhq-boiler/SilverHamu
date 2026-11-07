import unittest
from identity import classify

class CommitIdentity(unittest.TestCase):
    def classify(self, starts=(2,), ends=(7,), probe=3, sleep=(4,8)):
        return classify({"jbd2_start_commit": starts, "jbd2_end_commit": ends},
                        [(probe,"waiting",9)],[(probe,9)],[sleep],1,10)[1]

    def test_completion_while_blocked(self):
        self.assertEqual(self.classify(), "blocked_at_end")

    def test_completed_before_wait(self):
        self.assertEqual(self.classify(probe=8,sleep=(8,9)), "already_ended")

    def test_woken_before_completion(self):
        self.assertEqual(self.classify(sleep=(4,6)), "observed_end")

    def test_capture_edges_and_reused_ids(self):
        for starts,ends in [((),(7,)),((2,),()),((2,5),(4,7)),((6,),(5,))]:
            with self.subTest(starts=starts,ends=ends):
                self.assertEqual(self.classify(starts,ends),"unresolved")

if __name__=="__main__":
    unittest.main()
