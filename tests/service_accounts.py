"""Exercise the account protocol over real TLS connections and disposable databases."""
import json
import concurrent.futures
import os
import pathlib
import secrets
import socket
import ssl
import struct
import subprocess
import sys
import tempfile
import unittest

from service_protocol import BINARY, Client


class AccountsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = pathlib.Path(cls.tmp.name)
        cls.cert = cls.root / "cert.pem"
        cls.key = cls.root / "key.pem"
        config = cls.root / "openssl.cnf"
        config.write_text("[req]\ndistinguished_name=dn\n[dn]\n",encoding="ascii")
        subprocess.run([os.getenv("OPENSSL", "openssl"), "req", "-x509", "-newkey", "rsa:2048",
                        "-config", config,
                        "-nodes", "-days", "1", "-subj", "/CN=localhost",
                        "-addext", "subjectAltName=DNS:localhost", "-keyout", cls.key, "-out", cls.cert],
                       check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=self.root)
        self.addCleanup(self.temp.cleanup)
        work = pathlib.Path(self.temp.name)
        self.db = work / "main.ndb"
        self.second = work / "second.ndb"
        self.secret = secrets.token_urlsafe(32)
        self.file = work / "secret"
        self.file.write_text(self.secret)
        self.proc = None
        self.addCleanup(self.stop)
        self.start(["--db", self.second, "--init", "--token-file", self.file])
        self.stop()
        self.flags = ["--db", self.db, "--tls-cert", self.cert, "--tls-key", self.key,
                      "--attach", f"second={self.second}"]
        self.bootstrap = ["--bootstrap-admin", "owner", "--bootstrap-password-file", self.file]
        self.start([*self.flags, "--init", *self.bootstrap])

    def start(self, flags):
        self.proc = subprocess.Popen([BINARY, "--bind", "127.0.0.1:0", *flags],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        line = self.proc.stdout.readline()
        self.assertIn("catdb-server ready", line, self.proc.stderr.read() if not line else line)
        self.port = int(line.split()[2].rsplit(":", 1)[1])

    def stop(self):
        if self.proc is not None:
            if self.proc.poll() is None:
                self.proc.kill()
            self.proc.communicate(timeout=10)
            self.proc = None

    def login(self, user="owner", db="main", password=None):
        return Client("localhost", self.port, username=user, password=password or self.secret,
                      database=db, cafile=str(self.cert))

    @unittest.skipUnless(os.getenv("CATDB_SQL_HOST"), "requires the built Catdb.Sql.Host executable")
    def test_sql_uses_selected_database_admin_grant(self):
        self.stop()
        self.start([*self.flags, "--sql-host", os.environ["CATDB_SQL_HOST"]])
        with self.login() as admin, self.login(db="second") as second:
            self.assertTrue(admin.request(op="sql", sql="CREATE TABLE t(id INT)")["ok"])
            self.assertEqual(second.request(op="sql", sql="SHOW TABLES")["results"][0]["Rows"], [])
            for role in ("reader", "writer"):
                self.assertTrue(admin.request(op="create_user", username=role, password=self.secret, role=role)["ok"])
                with self.login(role) as limited:
                    denied = limited.request(op="sql", sql="CREATE TABLE blocked(id INT)", role="admin")
                    self.assertEqual(denied["error"], "forbidden")
                    self.assertFalse(denied["in_transaction"])
            self.assertTrue(admin.request(op="create_user", username="sqluser", password=self.secret, role="admin")["ok"])
            with self.login("sqluser") as changing:
                self.assertTrue(changing.request(op="sql", sql="BEGIN; INSERT INTO t VALUES (1)")["ok"])
                self.assertTrue(admin.request(op="set_role", username="sqluser", role="reader")["ok"])
                self.assertEqual(changing.request(op="sql", sql="COMMIT")["error"], "unauthorized")
            self.assertEqual(admin.request(op="sql", sql="SELECT COUNT(*) FROM t")["results"][0]["Rows"][0][0]["Text"], "0")

    def test_database_grants_and_revocation_of_staged_write(self):
        with self.login() as admin, self.login(db="second") as second:
            for name, role in (("r","reader"),("w","writer"),("a","admin")):
                self.assertTrue(admin.request(op="create_user", username=name, password=self.secret, role=role)["ok"])
            self.assertEqual(admin.request(op="create_user", username="r", role="admin",
                                           password=self.secret)["error"], "user_exists")
            self.assertFalse(admin.request(op="set_role", username="owner", role="none")["ok"])
            with self.assertRaises(RuntimeError):
                self.login("a", "second")
            self.assertTrue(second.request(op="set_role", username="r", role="writer")["ok"])
            with self.login("r") as r, self.login("w") as w, self.login("r","second") as other:
                self.assertEqual(r.account_id,other.account_id)
                self.assertNotEqual(r.account_id,w.account_id)
                self.assertTrue(w.request(op="put_commit",key=b"v1/bootstrap-principal".hex(),value=w.account_id)["ok"])
                self.assertFalse(w.request(op="set_role",username="w",role="admin")["ok"])
                self.assertTrue(other.request(op="put_commit",key="aa",value="01")["ok"])
                self.assertIsNone(r.request(op="get_latest",key="aa")["value"])
                for op in ("put","delete","put_commit","delete_commit","put_batch_commit",
                           "checkpoint","create_user","set_role","diagnostic_last","benchmark_stall"):
                    self.assertEqual(r.request(op=op)["error"], "forbidden")
                self.assertTrue(r.request(op="begin")["ok"])
                self.assertTrue(r.request(op="get",key="01")["ok"])
                self.assertTrue(r.request(op="commit")["ok"])
                self.assertFalse(w.request(op="checkpoint")["ok"])
                self.assertTrue(w.request(op="begin")["ok"])
                self.assertTrue(w.request(op="put",key="aa",value="02")["ok"])
                self.assertTrue(admin.request(op="set_role",username="w",role="reader")["ok"])
                self.assertEqual(w.request(op="commit")["error"],"unauthorized")
                self.assertIsNone(r.request(op="get_latest",key="aa")["value"])
            with self.login("a") as a:
                self.assertTrue(a.request(op="checkpoint")["ok"])
                self.assertFalse(a.request(op="set_role",username="r",role="admin",database="second")["ok"])
        self.stop()
        self.start(self.flags)
        with self.login("r","second") as other:
            self.assertEqual(other.request(op="get_latest",key="aa")["value"],"01")
        with self.login("w") as downgraded:
            self.assertFalse(downgraded.request(op="put_commit",key="aa",value="bb")["ok"])

    def test_tls_authentication_and_bootstrap_are_fail_closed(self):
        with self.assertRaises(ssl.SSLCertVerificationError):
            Client("127.0.0.1",self.port,username="owner",password=self.secret,cafile=str(self.cert))
        with self.assertRaises(ssl.SSLCertVerificationError):
            Client("localhost",self.port,username="owner",password=self.secret)
        for user,pwd in (("missing",self.secret),("owner","incorrect-password-value")):
            with self.assertRaises(RuntimeError):
                self.login(user,password=pwd)
        with socket.create_connection(("localhost",self.port),timeout=3) as sock:
            body=json.dumps({"op":"auth","version":2,"username":"owner","password":self.secret}).encode()
            sock.sendall(struct.pack("!I",len(body))+body)
            try: reply=sock.recv(1024)
            except ConnectionError: reply=b""
            self.assertNotIn(b'"ok":true',reply)
        with ssl.create_default_context(cafile=str(self.cert)).wrap_socket(
                socket.create_connection(("localhost",self.port)),server_hostname="localhost") as sock:
            client=Client.__new__(Client);client.socket=sock
            self.assertEqual(client.request(op="auth",version=1,token=self.secret)["error"],"unauthorized")
        self.stop()
        for flags in ([*self.flags,*self.bootstrap],["--db",self.db,"--token-file",self.file],
                      ["--db",self.second,"--token-file",self.file],
                      [*self.flags,"--token-file",self.file]):
            result=subprocess.run([BINARY,"--bind","127.0.0.1:0",*flags],timeout=5,capture_output=True)
            self.assertNotEqual(result.returncode,0)
        data=pathlib.Path(str(self.db)+".accounts").read_bytes()
        self.assertIn(b"$argon2id$",data)
        self.assertNotIn(self.secret.encode(),data)

    def test_concurrent_creation_and_password_limits(self):
        with self.login() as first, self.login() as second:
            def create(client):
                return client.request(op="create_user",username="shared",password=self.secret,role="reader")
            with concurrent.futures.ThreadPoolExecutor(2) as pool:
                results=list(pool.map(create,[first,second]))
            self.assertEqual(sum(r["ok"] for r in results),1)
            self.assertIn(next(r["error"] for r in results if not r["ok"]),("conflict","user_exists"))
            for password in ("", "短"*14, "a"*1025):
                self.assertEqual(first.request(op="create_user",username="bad",password=password,role="writer")["error"],"invalid_request")
            self.assertTrue(first.request(op="create_user",username="boundary",password="長"*15,role="reader")["ok"])
        with self.login("boundary",password="長"*15) as reader:
            self.assertTrue(reader.request(op="ping")["ok"])
        with self.assertRaises(RuntimeError):
            self.login(db="absent")

    def test_rate_limit_does_not_interrupt_existing_sessions(self):
        with self.login() as session:
            for _ in range(8):
                with self.assertRaises(RuntimeError):
                    self.login(password="incorrect-password-value")
            with self.assertRaises(RuntimeError):
                self.login()
            self.assertTrue(session.request(op="ping")["ok"])

    def test_missing_catalog_cannot_reset_an_enrolled_database(self):
        self.stop()
        catalog=pathlib.Path(str(self.db)+".accounts")
        catalog.rename(catalog.with_suffix(".saved"))
        for flags in ([*self.flags,*self.bootstrap],["--db",self.db,"--token-file",self.file]):
            run=subprocess.run([BINARY,"--bind","127.0.0.1:0",*flags],capture_output=True,timeout=5)
            self.assertNotEqual(run.returncode,0)
        self.assertFalse(catalog.exists())

    def test_cli_password_file_and_revoked_grant(self):
        with self.login() as admin:
            self.assertTrue(admin.request(op="create_user",username="probe",password=self.secret,role="reader")["ok"])
            cmd=[sys.executable,str(pathlib.Path(__file__).resolve().parents[1]/"scripts/catdb-client.py"),
                 "--host","localhost","--port",str(self.port),"--user","probe","--ca-file",str(self.cert),"--password-file",str(self.file)]
            run=subprocess.run(cmd,input='{"op":"ping"}\n',text=True,capture_output=True,timeout=10)
            self.assertEqual(run.returncode,0,run.stderr)
            self.assertTrue(json.loads(run.stdout)["ok"])
            self.assertNotIn(self.secret,run.stdout+run.stderr)
            self.assertTrue(admin.request(op="set_role",username="probe",role="none")["ok"])
        with self.assertRaises(RuntimeError):
            self.login("probe")


if __name__=="__main__":
    unittest.main()
