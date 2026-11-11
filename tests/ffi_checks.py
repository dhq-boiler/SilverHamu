import ctypes as c
import os
import sys
from pathlib import Path
import struct
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
NATIVE = c.CDLL(os.environ.get("NEWDBMS_TEST_DLL", str(ROOT / "target/release" / ("silverhamu.dll" if os.name == "nt" else "libsilverhamu.dylib" if sys.platform == "darwin" else "libsilverhamu.so"))))
U8 = c.POINTER(c.c_uint8)
U64 = c.c_uint64
NATIVE.ndb_open.argtypes = [U8, U64, U8, c.POINTER(U64), c.POINTER(U64)]
NATIVE.ndb_create.argtypes = [U8, U64, U8, U8, U64, U64]
NATIVE.ndb_read_page.argtypes = [U64, U64, c.c_uint32, U8, U64, c.POINTER(U64), c.POINTER(U64)]
NATIVE.ndb_close.argtypes = [U64]


def buf(value):
    return (c.c_uint8 * max(1, len(value)))(*value)


class FfiChecks(unittest.TestCase):
    def setUp(self):
        artifacts = (ROOT / "artifacts").resolve()
        artifacts.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="ffi-check-", dir=artifacts)
        self.root = Path(self.temp.name).resolve()
        assert self.root.is_relative_to(artifacts)
        self.path = str(self.root / "日本語.ndb").encode("utf-8")
        self.generation = bytes(range(16))
        self.handles = []

    def tearDown(self):
        for handle in self.handles:
            NATIVE.ndb_close(handle)
        self.temp.cleanup()

    def create(self, values):
        framed = b"".join(struct.pack("<I", len(v)) + v for v in values)
        return NATIVE.ndb_create(buf(self.path), len(self.path), buf(self.generation), buf(framed), len(framed), len(values))

    def open(self):
        handle, count = U64(), U64()
        code = NATIVE.ndb_open(buf(self.path), len(self.path), buf(self.generation), c.byref(handle), c.byref(count))
        if code == 0:
            self.handles.append(handle.value)
        return code, handle.value, count.value

    def test_eq1_unicode_path_roundtrip_and_buffer_retry(self):
        self.assertEqual(self.create([b"abc", b"", "日本語".encode()]), 0)
        code, handle, count = self.open()
        self.assertEqual((code, count), (0, 3))
        size, count = U64(), U64()
        self.assertEqual(NATIVE.ndb_read_page(handle, 0, 3, None, 0, c.byref(size), c.byref(count)), 3)
        target = (c.c_uint8 * size.value)()
        self.assertEqual(NATIVE.ndb_read_page(handle, 0, 3, target, len(target), c.byref(size), c.byref(count)), 0)
        expected = struct.pack("<I", 3) + b"abc" + struct.pack("<I", 0) + struct.pack("<I", 9) + "日本語".encode()
        self.assertEqual(bytes(target), expected)

    def test_bv1_empty_create_and_zero_page(self):
        self.assertEqual(self.create([]), 0)
        code, handle, count = self.open()
        self.assertEqual((code, count), (0, 0))
        size, count = U64(), U64()
        self.assertEqual(NATIVE.ndb_read_page(handle, 0, 0, None, 0, c.byref(size), c.byref(count)), 0)
        self.assertEqual((size.value, count.value), (0, 0))

    def test_st1_existing_file_and_invalid_handles_are_rejected(self):
        self.assertEqual(self.create([b"original"]), 0)
        self.assertNotEqual(self.create([b"replacement"]), 0)
        code, handle, _ = self.open()
        self.assertEqual(code, 0)
        self.assertEqual(NATIVE.ndb_close(handle), 0)
        self.assertNotEqual(NATIVE.ndb_close(handle), 0)
        size, count = U64(), U64()
        self.assertNotEqual(NATIVE.ndb_read_page(handle, 0, 1, None, 0, c.byref(size), c.byref(count)), 0)

    def test_st2_wrong_generation_and_null_output_are_rejected(self):
        self.assertEqual(self.create([b"record"]), 0)
        handle, count = U64(), U64()
        self.assertNotEqual(NATIVE.ndb_open(buf(self.path), len(self.path), buf(bytes(16)), c.byref(handle), c.byref(count)), 0)
        self.assertNotEqual(NATIVE.ndb_open(buf(self.path), len(self.path), buf(self.generation), None, c.byref(count)), 0)


if __name__ == "__main__":
    unittest.main()
