import os
import ctypes as c
import pathlib
import struct
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
DLL = c.CDLL(os.environ.get("NEWDBMS_TEST_DLL", str(ROOT / "target/release" / ("newdbms.dll" if os.name == "nt" else "libnewdbms.dylib" if sys.platform == "darwin" else "libnewdbms.so"))))
U64 = c.c_uint64
PTR = c.c_void_p
for name, args in {
    'db_create_limited': [PTR, U64, PTR, PTR, PTR],
    'db_open_limited': [PTR, U64, PTR, PTR],
    'db_close': [U64], 'begin': [U64, PTR, PTR], 'put': [U64, PTR, U64, PTR, U64],
    'commit': [U64, PTR], 'rollback': [U64], 'db_resources': [U64, PTR],
    'scan_page': [U64, PTR, U64, PTR, U64, c.c_uint32, PTR, U64, c.c_uint32,
                  U64, U64, U64, PTR, U64, PTR],
}.items():
    getattr(DLL, 'ndb_tx_' + name).argtypes = args


class Checks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='page-abi-', dir=ROOT / 'artifacts')
        self.path = str(pathlib.Path(self.temp.name) / 'source.wal').encode()
        self.db = U64()
        self.limits = (U64 * 5)(4, 1048576, 1048576, 1048576, 1048576)
        self.assertEqual(DLL.ndb_tx_db_create_limited(self.path, len(self.path), bytes(16), self.limits, c.byref(self.db)), 0)
        self.tx = U64(); version = U64()
        self.assertEqual(DLL.ndb_tx_begin(self.db, c.byref(self.tx), c.byref(version)), 0)
        self.assertEqual(DLL.ndb_tx_put(self.tx, b'', 0, b'', 0), 0)
        self.assertEqual(DLL.ndb_tx_put(self.tx, b'a', 1, b'one', 3), 0)

    def tearDown(self):
        DLL.ndb_tx_rollback(self.tx)
        DLL.ndb_tx_db_close(self.db)
        self.temp.cleanup()

    def scan(self, buffer=None, capacity=0, required=None, **kwargs):
        if required is None:
            required = c.byref(U64())
        return DLL.ndb_tx_scan_page(self.tx, b'', 0, None, kwargs.get('end_len', 0), kwargs.get('has_end', 0),
            kwargs.get('after', None), kwargs.get('after_len', 0), kwargs.get('has_after', 0),
            kwargs.get('rows', 1), 100, 10, buffer, capacity, required)

    def test_er_invalid_pointers_flags_limits_and_handles(self):
        self.assertEqual(self.scan(required=0), 1)
        self.assertEqual(self.scan(capacity=40), 1)
        self.assertEqual(self.scan(has_end=2), 1)
        self.assertEqual(self.scan(end_len=1, has_end=1), 1)
        self.assertEqual(self.scan(has_after=2), 1)
        self.assertEqual(self.scan(after_len=1, has_after=1), 1)
        self.assertEqual(self.scan(rows=0), 22)
        self.assertEqual(DLL.ndb_tx_db_resources(self.db, None), 1)
        self.assertEqual(DLL.ndb_tx_resource_api_version(), 1)
        saved = self.tx
        try:
            self.tx = U64(0)
            self.assertEqual(self.scan(), 1)
        finally:
            self.tx = saved

    def test_bv_buffer_query_retry_preserves_empty_cursor_and_no_leaks(self):
        required = U64()
        for _ in range(5):
            self.assertEqual(self.scan(required=c.byref(required)), 3)
            self.assertEqual(required.value, 48)
        small = c.create_string_buffer(47)
        self.assertEqual(self.scan(small, 47, c.byref(required)), 3)
        exact = c.create_string_buffer(48)
        self.assertEqual(self.scan(exact, 48, c.byref(required)), 0)
        self.assertEqual(struct.unpack('<5Q', exact.raw[:40]), (1, 1, 0, 1, 0))
        following = c.create_string_buffer(100)
        self.assertEqual(self.scan(following, 100, c.byref(required), after=b'', has_after=1), 0)
        self.assertEqual(struct.unpack('<5Q', following.raw[:40]), (1, 1, 4, 0, 0))
        self.assertEqual(following.raw[48:52], b'aone')
        usage = (U64 * 15)()
        self.assertEqual(DLL.ndb_tx_db_resources(self.db, usage), 0)
        self.assertEqual(usage[4], 0)
        self.assertEqual(usage[2], 96)

    def test_er_invalid_limits_do_not_create_files_or_return_handles(self):
        invalid = (U64 * 5)(0, 1, 1, 1, 1)
        path = str(pathlib.Path(self.temp.name) / 'never.wal').encode()
        output = U64(999)
        self.assertEqual(DLL.ndb_tx_db_create_limited(path, len(path), bytes(16), invalid, c.byref(output)), 1)
        self.assertEqual(output.value, 0)
        self.assertFalse(pathlib.Path(path.decode()).exists())


if __name__ == '__main__':
    unittest.main()
