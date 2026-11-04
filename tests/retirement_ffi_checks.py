import unittest
from transaction_ffi_checks import Checks, N, U64, c

N.ndb_tx_set_checkpoint_retirement.argtypes = [U64, c.c_uint32, U64]
N.ndb_tx_checkpoint_retirement_status.argtypes = [U64, c.POINTER(U64)]
N.ndb_tx_checkpoint_deferred.argtypes = [U64] + [c.POINTER(U64)] * 4
N.ndb_tx_reclaim_checkpoint.argtypes = [U64]


class RetirementChecks(Checks):
    def test_deferred_invalid_outputs_and_stale_handle(self):
        values = [U64(99) for _ in range(4)]
        self.assertEqual(N.ndb_tx_checkpoint_deferred(0, *[c.byref(v) for v in values]), 1)
        self.assertEqual([v.value for v in values], [0] * 4)
        for index in range(4):
            pointers = [c.byref(v) for v in values]
            pointers[index] = None
            self.assertEqual(N.ndb_tx_checkpoint_deferred(self.db, *pointers), 1)
        self.assertEqual(N.ndb_tx_reclaim_checkpoint(0), 1)
        self.assertEqual(N.ndb_tx_reclaim_checkpoint(self.db), 0)

    def test_invalid_inputs_preserve_settings_and_clear_status_outputs(self):
        values = (U64 * 8)()
        self.assertEqual(N.ndb_tx_set_checkpoint_retirement(self.db, 1, 100), 0)
        self.assertEqual(N.ndb_tx_set_checkpoint_retirement(self.db, 2, 100), 1)
        self.assertNotEqual(N.ndb_tx_set_checkpoint_retirement(self.db, 1, 1024**3 + 1), 0)
        self.assertEqual(N.ndb_tx_checkpoint_retirement_status(self.db, values), 0)
        self.assertEqual(list(values)[:2], [1, 100])
        self.assertEqual(N.ndb_tx_checkpoint_retirement_status(self.db, None), 1)
        self.assertEqual(N.ndb_tx_set_checkpoint_retirement(0, 1, 100), 1)
        self.assertEqual(N.ndb_tx_checkpoint_retirement_status(0, values), 1)
        self.assertEqual(list(values), [0] * 8)


if __name__ == '__main__':
    unittest.main()
