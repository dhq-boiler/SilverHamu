"""End-to-end WAL selection, explicit migration and persisted policy checks."""
from contextlib import contextmanager
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
import zlib
import service_protocol as protocol


class ManagedWalServiceTests(unittest.TestCase):
    def setUp(self):
        folder = self.enterContext(tempfile.TemporaryDirectory())
        self.root = Path(folder)
        self.secret = self.root / "token"
        self.token = "9" * 64
        self.secret.write_text(self.token)

    def command(self, path):
        return [str(protocol.BINARY), "--db", str(path), "--init", "--bind", "127.0.0.1:0",
                "--token-file", str(self.secret)]

    def environment(self, overrides):
        env = dict(os.environ)
        for name in ("CATDB_MIGRATE_WAL", "CATDB_EXPERIMENTAL_FIXED_LOG_BYTES", "CATDB_EXPERIMENTAL_REDO_LOG_BYTES"):
            env.pop(name, None)
        env.update(overrides)
        return env

    @contextmanager
    def server(self, path, **options):
        process = subprocess.Popen(self.command(path), env=self.environment(options), stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        try:
            line = process.stdout.readline()
            self.assertIn("ready", line, process.stderr.read() if not line else line)
            port = int(line.split()[2].rsplit(":", 1)[-1])
            with protocol.Client("127.0.0.1", port, self.token) as client:
                yield client
        finally:
            process.kill()
            process.wait(timeout=10)
            process.stdout.close()
            process.stderr.close()

    def layout(self, path):
        with path.open("rb") as f:
            data = f.read(48)
        self.assertEqual(zlib.crc32(data[:44]), int.from_bytes(data[44:], "little"))
        return int.from_bytes(data[8:12],"little"), bool(data[43] & 128), data[16:32]

    def test_existing_layout_requires_explicit_conversion_and_preserves_id_values_and_version(self):
        for version in (1, 2, 3, 4):
            with self.subTest(version=version):
                path = self.root / f"source-{version}.ndb"
                options = {}
                if version in (1, 2):
                    header = struct.pack("<8sII16sQI", b"NDTXN001", version, 48, bytes(range(16)), 0, 0)
                    path.write_bytes(header + struct.pack("<I", zlib.crc32(header)))
                else:
                    key = "CATDB_EXPERIMENTAL_FIXED_LOG_BYTES" if version == 3 else "CATDB_EXPERIMENTAL_REDO_LOG_BYTES"
                    options[key] = "1048576"
                with self.server(path, **options) as c:
                    self.assertEqual(c.request(op="put_commit", key="0102", value="fabe")["version"], "1")
                source = self.layout(path)
                self.assertEqual(source[:2], (version, False))
                with self.server(path) as c:
                    self.assertEqual(c.request(op="get_latest", key="0102")["value"], "fabe")
                self.assertEqual(self.layout(path), source)
                with self.server(path, CATDB_MIGRATE_WAL="managed") as c:
                    self.assertEqual(c.request(op="get_latest", key="0102")["value"], "fabe")
                    self.assertEqual(c.request(op="put_commit", key="0203", value="aa")["version"], "2")
                self.assertEqual(self.layout(path), (4, True, source[2]))
                captured = path.read_bytes()
                with self.server(path, CATDB_MIGRATE_WAL="managed") as c:
                    self.assertEqual(c.request(op="get_latest", key="0203")["value"], "aa")
                self.assertEqual(captured, path.read_bytes(), "already managed open should not checkpoint again")

    def test_incompatible_migration_options_are_rejected_without_modifying_the_existing_file(self):
        path = self.root / "managed.ndb"
        with self.server(path) as c:
            self.assertTrue(c.request(op="put_commit", key="01", value="02")["ok"])
        before = path.read_bytes()
        for options in [{"CATDB_MIGRATE_WAL":"typo"},
                        {"CATDB_MIGRATE_WAL":"managed","CATDB_EXPERIMENTAL_FIXED_LOG_BYTES":"1048576"},
                        {"CATDB_MIGRATE_WAL":"managed","CATDB_EXPERIMENTAL_REDO_LOG_BYTES":"1048576"}]:
            with self.subTest(options=options):
                proc=subprocess.run(self.command(path), capture_output=True, timeout=10, env=self.environment(options))
                self.assertNotEqual(proc.returncode, 0)
                self.assertEqual(path.read_bytes(), before)

if __name__=="__main__":
    unittest.main(verbosity=2)
