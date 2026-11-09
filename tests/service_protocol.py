"""Protocol, ownership, durability and bounded network tests against a real process."""
import concurrent.futures
import importlib.util
import json
import os
import pathlib
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("catdb_client", ROOT / "scripts/catdb-client.py")
client_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client_module)
Client = client_module.Client
BINARY = pathlib.Path(os.environ.get("CATDB_SERVER", ROOT / "target/debug" / ("catdb-server.exe" if os.name == "nt" else "catdb-server")))


class ServiceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / "artifacts").mkdir(exist_ok=True)
        cls.root = pathlib.Path(tempfile.mkdtemp(prefix="service-protocol-", dir=ROOT / "artifacts"))
        cls.token = secrets.token_hex(32)
        cls.secret = cls.root / "token"
        cls.secret.write_text(cls.token)

    def setUp(self):
        self.folder = self.root / self._testMethodName
        self.folder.mkdir()
        self.db = self.folder / "state.ndb"
        self.process = None
        self.clients = []
        self.start()

    def start(self, *extra):
        self.process = subprocess.Popen([str(BINARY), "--db", str(self.db), "--init", "--bind", "127.0.0.1:0", "--token-file", str(self.secret), *extra], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        line = self.process.stdout.readline()
        self.assertIn("ready", line, self.process.stderr.read() if not line else line)
        self.port = int(line.split()[2].rsplit(":", 1)[1])

    def stop(self):
        if self.process:
            self.process.kill()
            self.process.wait(timeout=10)
            self.process.stdout.close()
            self.process.stderr.close()
            self.process = None

    def tearDown(self):
        for client in self.clients:
            client.close()
        self.stop()

    def client(self):
        client = Client("127.0.0.1", self.port, self.token)
        self.clients.append(client)
        return client

    def ok(self, client, **request):
        result = client.request(**request)
        self.assertTrue(result["ok"], result)
        return result

    def seed(self, key="01", value="ff"):
        with Client("127.0.0.1", self.port, self.token) as c:
            self.ok(c, op="begin")
            self.ok(c, op="put", key=key, value=value)
            return self.ok(c, op="commit")

    @unittest.skipUnless(os.getenv("CATDB_SQL_HOST"), "requires the built Catdb.Sql.Host executable")
    def test_sql_host_and_view_token(self):
        self.stop()
        read_token = secrets.token_hex(32)
        read_secret = self.folder / "read-token"
        read_secret.write_text(read_token, encoding="utf-8")
        self.start("--sql-host", os.environ["CATDB_SQL_HOST"], "--read-role", "reader", "--read-token-file", str(read_secret))
        with Client("127.0.0.1", self.port, self.token) as admin, Client("127.0.0.1", self.port, read_token) as viewer:
            self.ok(admin, op="sql", sql="CREATE TABLE docs(id INT PRIMARY KEY); INSERT INTO docs VALUES (1); CREATE VIEW visible AS SELECT id FROM docs; GRANT SELECT ON visible TO reader")
            self.ok(admin, op="begin")
            self.assertTrue(self.ok(admin, op="sql", sql="INSERT INTO docs VALUES (2)")["in_transaction"])
            self.ok(admin, op="rollback")
            self.ok(admin, op="sql", sql="START TRANSACTION; SAVEPOINT keep_docs")
            self.ok(admin, op="sql", sql="DELETE FROM docs")
            self.assertTrue(self.ok(admin, op="sql", sql="ROLLBACK TO SAVEPOINT keep_docs")["in_transaction"])
            self.assertFalse(self.ok(admin, op="sql", sql="RELEASE SAVEPOINT keep_docs; COMMIT")["in_transaction"])
            rows = self.ok(viewer, op="sql", sql="SELECT id FROM visible")["results"][0]["Rows"]
            self.assertEqual([row[0]["Text"] for row in rows], ["1"])
            self.assertFalse(viewer.request(op="sql", role="admin", sql="SELECT id FROM docs")["ok"])
            self.assertEqual(viewer.request(op="get_latest", key="")["error"], "forbidden")
            self.ok(admin, op="sql", sql="REVOKE SELECT ON visible FROM reader")
            self.assertFalse(viewer.request(op="sql", sql="SELECT id FROM visible")["ok"])

    def test_st_read_only_snapshot_rejects_mutations_and_survives_updates(self):
        self.seed(); c=self.client(); self.ok(c,op="begin_read")
        self.assertFalse(c.request(op="put",key="01",value="aa")["ok"])
        self.assertFalse(c.request(op="delete",key="01")["ok"])
        self.seed(value="bb")
        self.assertEqual(self.ok(c,op="get",key="01")["value"],"ff")
        self.ok(c,op="commit")
        self.assertEqual(self.ok(c,op="get_latest",key="01")["value"],"bb")

    def test_eq_single_requests_crud_pages_ping_and_reopen(self):
        c = self.client()
        self.assertIsNone(self.ok(c, op="get_latest", key="01")["value"])
        self.assertEqual(self.ok(c, op="scan_latest", start="")["rows"], [])
        for k, v in [("01", ""), ("02", "abcd"), ("03", "ff")]:
            result = self.ok(c, op="put_commit", key=k, value=v)
            self.assertEqual(self.ok(c, op="ping")["version"], result["version"])
        self.assertEqual(self.ok(c, op="get_latest", key="01")["value"], "")
        old = self.client(); self.ok(old, op="begin")
        self.ok(old, op="get", key="02")
        page = self.ok(c, op="scan_latest", start="", rows=2)
        self.assertEqual(page["rows"], [["01", ""], ["02", "abcd"]])
        self.ok(c, op="put_commit", key="03", value="aa")
        self.assertEqual(self.ok(c, op="scan_latest", start="", after=page["next_after"])["rows"], [["03", "aa"]])
        self.ok(c, op="delete_commit", key="02")
        self.assertEqual(self.ok(old, op="get", key="02")["value"], "abcd")
        self.ok(old, op="put", key="04", value="bb")
        self.assertEqual(old.request(op="commit")["error"], "conflict")
        self.stop(); self.start(); c = self.client()
        self.assertEqual(self.ok(c, op="scan_latest", start="")["rows"], [["01", ""], ["03", "aa"]])

    def test_st_single_requests_reject_active_transaction_without_discarding_it(self):
        c = self.client(); self.ok(c, op="begin"); self.ok(c, op="put", key="01", value="aa")
        for op in ["get_latest", "scan_latest", "put_commit", "delete_commit"]:
            self.assertEqual(c.request(op=op, key="01", value="bb", start="")["error"], "invalid_request")
            self.assertEqual(self.ok(c, op="get", key="01")["value"], "aa")
        self.ok(c, op="commit")
        self.assertEqual(self.ok(c, op="get_latest", key="01")["value"], "aa")

    def test_bv_single_requests_limits_leave_connection_and_data_intact(self):
        c = self.client()
        for request in [dict(op="put_commit",key="ab"*1025,value=""), dict(op="put_commit",key="01",value="ab"*(256*1024+1)), dict(op="scan_latest",start="",rows=0), dict(op="scan_latest",start="",rows=4097), dict(op="put_commit",key="zz",value="ab")]:
            self.assertFalse(c.request(**request)["ok"])
            self.ok(c, op="ping")
        self.assertEqual(self.ok(c, op="scan_latest", start="")["rows"], [])
        self.ok(c, op="put_commit", key="ab"*1024, value="ab"*(256*1024))
        self.assertFalse(c.request(op="scan_latest",start="",bytes=10)["ok"])
        self.assertEqual(len(self.ok(c, op="get_latest",key="ab"*1024)["value"]),256*1024*2)
        self.ok(c, op="delete_commit", key="ab"*1024)
        for count in [1,4096]: self.assertEqual(self.ok(c,op="scan_latest",start="",rows=count)["rows"],[])

    def test_er_single_request_acknowledgements_survive_process_exit(self):
        def writer(i):
            with Client("127.0.0.1",self.port,self.token) as c:
                for n in range(15): self.ok(c,op="put_commit",key=f"{i:02x}{n:02x}",value="ff")
        with concurrent.futures.ThreadPoolExecutor(4) as pool: list(pool.map(writer,range(4)))
        self.stop(); self.start()
        self.assertEqual(len(self.ok(self.client(),op="scan_latest",start="",rows=100)["rows"]),60)

    def test_eq_empty_value_missing_update_delete_and_restart(self):
        c = self.client()
        self.ok(c, op="begin")
        self.assertIsNone(self.ok(c, op="get", key="01")["value"])
        self.ok(c, op="put", key="01", value="")
        self.assertEqual(self.ok(c, op="get", key="01")["value"], "")
        self.ok(c, op="commit")
        self.seed(value="abcd")
        self.stop(); self.start()
        c = self.client(); self.ok(c, op="begin")
        self.assertEqual(self.ok(c, op="get", key="01")["value"], "abcd")
        self.ok(c, op="delete", key="01"); self.ok(c, op="commit")
        self.stop(); self.start()
        c = self.client(); self.ok(c, op="begin")
        self.assertIsNone(self.ok(c, op="get", key="01")["value"])

    def test_st_old_generation_read_and_same_key_conflicts(self):
        self.seed()
        old = self.client(); self.ok(old, op="begin")
        self.assertEqual(self.ok(old, op="get", key="01")["value"], "ff")
        self.seed(value="aa")
        self.assertEqual(self.ok(old, op="get", key="01")["value"], "ff")
        self.ok(old, op="put", key="03", value="01")
        self.assertEqual(old.request(op="commit")["error"], "conflict")
        a, b = self.client(), self.client()
        for c in [a, b]:
            self.ok(c, op="begin"); self.ok(c, op="put", key="02", value="ab")
        self.ok(a, op="commit")
        self.assertEqual(b.request(op="commit")["error"], "conflict")
        self.ok(b, op="begin")

    def test_st_range_conflict_and_paged_snapshot_across_checkpoint(self):
        for i in range(5): self.seed(f"{i:02x}")
        c = self.client(); self.ok(c, op="begin")
        first = self.ok(c, op="scan", start="", rows=2)
        self.assertEqual(len(first["rows"]), 2)
        self.seed("07")
        with Client("127.0.0.1", self.port, self.token) as other:
            self.ok(other, op="checkpoint")
        rows = first["rows"]; after = first["next_after"]
        while after is not None:
            page = self.ok(c, op="scan", start="", rows=2, after=after)
            rows.extend(page["rows"]); after = page["next_after"]
        self.assertEqual([row[0] for row in rows], [f"{i:02x}" for i in range(5)])
        self.ok(c, op="put", key="ff", value="01")
        self.assertEqual(c.request(op="commit")["error"], "conflict")

    def test_st_disconnect_discards_staged_values(self):
        c = self.client(); self.ok(c, op="begin"); self.ok(c, op="put", key="01", value="02"); c.close()
        c = self.client(); self.ok(c, op="begin")
        self.assertIsNone(self.ok(c, op="get", key="01")["value"])

    def test_er_wrong_auth_and_protocol_are_rejected(self):
        for request in [{"op":"auth","version":1,"token":"wrong"}, {"op":"auth","version":2,"token":self.token}, {"op":"ping"}]:
            with socket.create_connection(("127.0.0.1",self.port),2) as sock:
                body=json.dumps(request).encode();sock.sendall(struct.pack("!I",len(body))+body)
                head=sock.recv(4);size=struct.unpack("!I",head)[0];body=b""
                while len(body)<size: body+=sock.recv(size-len(body))
                self.assertEqual(json.loads(body)["error"],"unauthorized")
                self.assertEqual(sock.recv(1),b"")

    def test_bv_invalid_frames_and_limits_do_not_mutate(self):
        for length,body in [(0,b""),(1024*1024+1,b""),(1,b"{")]:
            with socket.create_connection(("127.0.0.1",self.port),2) as sock:
                sock.sendall(struct.pack("!I",length)+body)
                try: self.assertEqual(sock.recv(1),b"")
                except ConnectionResetError: pass
        c=self.client();self.ok(c,op="begin")
        for rows in [0,4097]: self.assertEqual(c.request(op="scan",start="",rows=rows)["error"],"limit")
        for rows in [1,4095,4096]: self.assertEqual(self.ok(c,op="scan",start="",rows=rows)["rows"],[])
        self.assertFalse(c.request(op="put",key="aa"*1025,value="")["ok"])
        self.ok(c,op="put",key="aa"*1024,value="")
        self.ok(c,op="put",key="01",value="ff"*(256*1024))
        self.assertEqual(c.request(op="put",key="01",value="ff"*(256*1024+1))["error"],"limit")
        self.assertEqual(c.request(op="scan",start="",bytes=10)["error"],"limit")
        self.assertEqual(len(self.ok(c,op="get",key="01")["value"]),256*1024*2)

    def test_bv_request_frame_accepts_exact_maximum_and_previous_byte(self):
        c=self.client()
        base=len(json.dumps({"op":"ping","padding":""},separators=(",",":")).encode())
        for size in [1024*1024-1,1024*1024]:
            self.ok(c,op="ping",padding="x"*(size-base))

    def test_er_remote_bind_requires_explicit_opt_in_and_token_file_is_bounded(self):
        args=[str(BINARY),"--db",str(self.folder/"remote.ndb"),"--init","--token-file",str(self.secret),"--bind","0.0.0.0:0"]
        run=subprocess.run(args,capture_output=True,text=True,timeout=10)
        self.assertNotEqual(run.returncode,0);self.assertFalse((self.folder/"remote.ndb").exists())
        oversized=self.folder/"oversized-token";oversized.write_text("x"*256+"\n"*10)
        args=[str(BINARY),"--db",str(self.folder/"remote.ndb"),"--init","--token-file",str(oversized),"--bind","127.0.0.1:0"]
        self.assertNotEqual(subprocess.run(args,capture_output=True,timeout=10).returncode,0)

    def test_bv_connection_cap_and_slot_reuse(self):
        self.stop();self.start("--max-connections","2")
        a,b=self.client(),self.client()
        with self.assertRaises((OSError,ConnectionError)): self.client()
        a.close()
        deadline=time.monotonic()+3
        while True:
            try: c=self.client();break
            except (OSError,ConnectionError):
                if time.monotonic()>deadline: raise
                time.sleep(.02)
        self.ok(c,op="ping"); self.ok(b,op="ping")

    def test_bv_transaction_deadline_is_not_reset_by_requests(self):
        self.stop();self.start("--transaction-timeout-secs","1")
        c=self.client();self.ok(c,op="begin"); self.ok(c,op="put",key="01",value="aa")
        time.sleep(.6);self.ok(c,op="ping");time.sleep(.6)
        with self.assertRaises((OSError,ConnectionError)): c.request(op="commit")
        c=self.client();self.ok(c,op="begin");self.assertIsNone(self.ok(c,op="get",key="01")["value"])

    def test_bv_slow_partial_frame_times_out(self):
        self.stop();self.start("--request-timeout-secs","1")
        with socket.create_connection(("127.0.0.1",self.port),3) as s:
            s.sendall(b"\0");time.sleep(1.3)
            self.assertEqual(s.recv(1),b"")

    def test_cc_four_writers_acknowledged_commits_survive_kill(self):
        def writer(index):
            with Client("127.0.0.1",self.port,self.token) as c:
                for n in range(15):
                    self.ok(c,op="begin");self.ok(c,op="put",key=f"{index:02x}{n:02x}",value="ff");self.ok(c,op="commit")
        with concurrent.futures.ThreadPoolExecutor(4) as pool: list(pool.map(writer,range(4)))
        staged=self.client();self.ok(staged,op="begin");self.ok(staged,op="put",key="ffff",value="00")
        self.stop();self.start()
        c=self.client();self.ok(c,op="begin")
        self.assertEqual(len(self.ok(c,op="scan",start="",rows=100)["rows"]),60)
        self.assertIsNone(self.ok(c,op="get",key="ffff")["value"])

    def test_er_second_owner_and_corrupt_existing_file_refused(self):
        args=[str(BINARY),"--db",str(self.db),"--init","--token-file",str(self.secret),"--bind","127.0.0.1:0"]
        run=subprocess.run(args,capture_output=True,text=True,timeout=10)
        self.assertNotEqual(run.returncode,0)
        self.stop(); self.db.write_bytes(b"corrupt")
        run=subprocess.run(args,capture_output=True,text=True,timeout=10)
        self.assertNotEqual(run.returncode,0);self.assertEqual(self.db.read_bytes(),b"corrupt")


    def test_bv_resource_arguments_reject_invalid_values_without_touching_database(self):
        self.seed(); self.stop(); original=self.db.read_bytes()
        args=[str(BINARY),"--db",str(self.db),"--init","--bind","127.0.0.1:0","--token-file",str(self.secret)]
        for name in ["retained","staged","dependency","buffer"]:
            for value in ["0","-1","18446744073709551616","nonsense"]:
                with self.subTest(name=name,value=value):
                    result=subprocess.run([*args,"--max-"+name+"-bytes",value],capture_output=True,text=True,timeout=10)
                    self.assertNotEqual(result.returncode,0)
                    self.assertEqual(self.db.read_bytes(),original)
            result=subprocess.run([*args,"--max-"+name+"-bytes"],capture_output=True,text=True,timeout=10)
            self.assertNotEqual(result.returncode,0);self.assertEqual(self.db.read_bytes(),original)

    def test_bv_retained_admission_boundary_and_reopen_refusal_preserve_values(self):
        self.stop()
        for budget in [258,259,260]:
            with self.subTest(budget=budget):
                self.db=self.folder / f"limit-{budget}.ndb"
                self.start("--max-retained-bytes",str(budget));c=self.client()
                result=c.request(op="put_commit",key="01",value="ff")
                if budget==258:
                    self.assertEqual(result["error"],"resource_exhausted")
                    self.assertIsNone(self.ok(c,op="get_latest",key="01")["value"])
                else:
                    self.assertTrue(result["ok"],result)
                    self.assertEqual(self.ok(c,op="get_latest",key="01")["value"],"ff")
                self.stop()
        original=self.db.read_bytes()
        args=[str(BINARY),"--db",str(self.db),"--bind","127.0.0.1:0","--token-file",str(self.secret),"--max-retained-bytes","258"]
        result=subprocess.run(args,capture_output=True,text=True,timeout=10)
        self.assertNotEqual(result.returncode,0);self.assertIn("resource exhausted",result.stderr)
        self.assertEqual(self.db.read_bytes(),original)
        self.start("--max-retained-bytes","1048576");c=self.client()
        old=self.client();self.ok(old,op="begin_read")
        self.ok(c,op="put_commit",key="01",value="aa")
        self.assertEqual(self.ok(old,op="get",key="01")["value"],"ff")
        self.stop();self.start("--max-retained-bytes","1048576");c=self.client()
        self.assertEqual(self.ok(c,op="get_latest",key="01")["value"],"aa")

    def test_st_staged_limit_refusal_preserves_prior_changes_and_releases_budget(self):
        self.stop();self.start("--max-staged-bytes","256");c=self.client()
        self.ok(c,op="begin");self.ok(c,op="put",key="01",value="ff")
        self.assertEqual(c.request(op="put",key="01",value="aa"*1024)["error"],"resource_exhausted")
        self.assertEqual(self.ok(c,op="get",key="01")["value"],"ff")
        self.ok(c,op="commit");self.ok(c,op="put_commit",key="02",value="bb")

    def test_st_dependency_limit_refusal_releases_on_rollback(self):
        self.stop();self.start("--max-dependency-bytes","97");c=self.client()
        self.ok(c,op="begin");self.ok(c,op="get",key="01")
        self.assertEqual(c.request(op="get",key="02")["error"],"resource_exhausted")
        self.ok(c,op="get",key="01");self.ok(c,op="rollback")
        self.ok(c,op="begin");self.ok(c,op="get",key="02");self.ok(c,op="rollback")

    def test_er_buffer_refusal_does_not_break_following_reads(self):
        self.seed();self.stop();self.start("--max-buffer-bytes","1024");c=self.client()
        self.assertEqual(c.request(op="put_commit",key="01",value="aa"*8192)["error"],"resource_exhausted")
        self.assertIsNone(self.ok(c,op="get_latest",key="02")["value"])
        self.stop();self.start();c=self.client()
        self.assertEqual(self.ok(c,op="get_latest",key="01")["value"],"ff")


if __name__ == "__main__":
    unittest.main(verbosity=2)
