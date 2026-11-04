"""Existing C ABI guarantees also hold with group commit enabled."""
import concurrent.futures
import threading
import unittest
from transaction_ffi_checks import Checks, N, U64, buf, c

N.ndb_tx_set_group_commit.argtypes = [U64, c.c_uint32]


class GroupChecks(Checks):
    def setUp(self):
        super().setUp()
        self.assertEqual(N.ndb_tx_set_group_commit(self.db, 1), 0)

    def test_st_parallel_group_updates_preserve_old_reads_and_survive_reopen(self):
        old = self.begin()
        for round_index in range(4):
            transactions = [self.begin() for _ in range(8)]
            barrier = threading.Barrier(8)

            def commit(item):
                lane, tx = item
                key = bytes([lane])
                value = bytes([round_index])
                put = N.ndb_tx_put(tx, buf(key), len(key), buf(value), len(value))
                barrier.wait(timeout=10)
                version = U64()
                result = N.ndb_tx_commit(tx, c.byref(version))
                return put, result, version.value

            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                results = list(pool.map(commit, enumerate(transactions)))
            self.assertTrue(all(put == result == 0 for put, result, _ in results))
            self.assertEqual(len({version for _, _, version in results}), 8)
            reader = self.begin()
            for lane in range(8):
                key, output = buf(bytes([lane])), buf(bytes(1))
                required, found = U64(), U64()
                self.assertEqual(N.ndb_tx_get(old, key, 1, output, 1, c.byref(required), c.byref(found)), 0)
                self.assertEqual(found.value, 0)
                self.assertEqual(N.ndb_tx_get(reader, key, 1, output, 1, c.byref(required), c.byref(found)), 0)
                self.assertEqual((found.value, bytes(output)), (1, bytes([round_index])))
        for tx in self.txs:
            N.ndb_tx_rollback(tx)
        self.txs.clear()
        self.assertEqual(N.ndb_tx_db_close(self.db), 0)
        reopened = U64()
        self.assertEqual(N.ndb_tx_db_open(buf(self.path), len(self.path), c.byref(reopened)), 0)
        self.db = reopened.value
        reader = self.begin()
        for lane in range(8):
            output, required, found = buf(bytes(1)), U64(), U64()
            self.assertEqual(N.ndb_tx_get(reader, buf(bytes([lane])), 1, output, 1, c.byref(required), c.byref(found)), 0)
            self.assertEqual((found.value, bytes(output)), (1, bytes([3])))


if __name__ == '__main__':
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(GroupChecks)
    outcome = unittest.TextTestRunner().run(suite)
    raise SystemExit(not outcome.wasSuccessful())
