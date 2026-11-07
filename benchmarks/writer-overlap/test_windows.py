import unittest

from attribute import volume
from exposure_report import split


class WindowTests(unittest.TestCase):
    def test_adjacent_windows_count_each_submission_once(self):
        times = [10, 20, 20, 30]
        prefix = [0, 2, 5, 12, 23]
        self.assertEqual(volume(times, prefix, 0, 10), 0)
        self.assertEqual(volume(times, prefix, 10, 20), 2)
        self.assertEqual(volume(times, prefix, 20, 30), 10)
        self.assertEqual(volume(times, prefix, 20, 20), 0)

    def test_fixed_duration_groups_isolate_frequency_change(self):
        quiet = {"prior_B": 0, "sync_ns": 1_000_000_000}
        busy = {"prior_B": 10, "sync_ns": 3_000_000_000}
        self.assertEqual(split([quiet, quiet, busy], [quiet, busy, busy]), (2, 0))


if __name__ == "__main__":
    unittest.main()
