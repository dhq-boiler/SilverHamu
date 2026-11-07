import unittest

from campaign import MIB, WINDOW, burst, passes


class CheckpointBurstTests(unittest.TestCase):
    def test_empty_publication_has_no_submissions(self):
        self.assertEqual(burst([]), (0, 0))

    def test_rolling_peak_crosses_bins_without_counting_a_large_fixed_bin(self):
        submissions = [(WINDOW - 1, 20 * MIB), (WINDOW + 1, 20 * MIB)]
        self.assertEqual(burst(submissions), (40 * MIB, 0))

    def test_endpoint_leaves_rolling_window_and_32_mib_counts_once(self):
        submissions = [(0, 16 * MIB), (0, 16 * MIB), (WINDOW, 8 * MIB)]
        self.assertEqual(burst(submissions), (32 * MIB, 32 * MIB))

    def test_gate_requires_both_locks_and_comparable_volume(self):
        trials = [
            dict(mode=mode, variant=variant, peak10_MiB=peak,
                 bulk_pct=bulk, submitted_B=10000)
            for mode in ("standard", "fifo")
            for variant, peak, bulk in (("baseline", 100, 95), ("range8", 80, 80))
        ]
        self.assertTrue(passes(trials))
        trials[-1]["peak10_MiB"] = 81
        self.assertFalse(passes(trials))
        trials[-1].update(peak10_MiB=80, submitted_B=10201)
        self.assertFalse(passes(trials))


if __name__ == "__main__":
    unittest.main()
