import unittest
from analyze_overtakes import summarize_attempts

class Overtakes(unittest.TestCase):
    def test_later_attempts_and_tied_arrivals_have_distinct_meanings(self):
        def attempt(key,received,start,admit,sync,end,version):
            return (key,received,start,start,start,admit,sync,end if version else 0,1 if version else 0,end,version)
        attempts=[
            attempt(1,0,1,2,3,12,1),
            attempt(3,11,11,14,0,16,0),
            attempt(2,10,10,20,21,30,2),
            attempt(3,11,39,40,45,50,3),
            attempt(0,9,10,100,101,102,4),
            attempt(4,103,103,104,105,106,5),
        ]
        row=next(r for r in summarize_attempts(attempts,5) if r["request"]==0)
        self.assertEqual((row["overtakes"],row["distinct"],row["passing_commits"]),(2,1,1))
        self.assertAlmostEqual(row["all_sync_ms"]*1e6,16)
        self.assertAlmostEqual(row["passing_sync_ms"]*1e6,5)
        self.assertAlmostEqual(row["longest_sync_ms"]*1e6,9)

    def test_registrations_at_sixteen_milliseconds_count_in_late_bucket(self):
        cutoff=16_000_000
        attempts=[(0,0,0,0,0,cutoff+8,cutoff+9,cutoff+10,1,cutoff+11,3),
                  (1,1,1,1,cutoff-1,cutoff,cutoff+1,cutoff+2,1,cutoff+3,1),
                  (2,2,2,2,cutoff,cutoff+4,cutoff+5,cutoff+6,1,cutoff+7,2)]
        row=next(r for r in summarize_attempts(attempts,3) if r["request"]==0)
        self.assertEqual((row["overtakes"],row["after16"]),(2,1))

if __name__=="__main__":
    unittest.main()
