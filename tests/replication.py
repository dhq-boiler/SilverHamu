"""Exercise elections, quorum loss and crash recovery using independent server processes."""
from contextlib import ExitStack, contextmanager
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import unittest

from service_protocol import BINARY, Client


@contextmanager
def suspend(process):
    if os.name == "nt":
        import ctypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        # Holding the initial debugger event suspends this test-owned child and preserves its sockets.
        if not kernel.DebugActiveProcess(process.pid):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            yield
        finally:
            if not kernel.DebugActiveProcessStop(process.pid):
                raise ctypes.WinError(ctypes.get_last_error())
    else:
        os.kill(process.pid, signal.SIGSTOP)
        try:
            yield
        finally:
            os.kill(process.pid, signal.SIGCONT)


class ClusterFixture(unittest.TestCase):
    member_count = 3

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="silverhamu-replication-")
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        with ExitStack() as reservations:
            ports = []
            for _ in range(2 * self.member_count):
                sock = reservations.enter_context(socket.socket())
                sock.bind(("127.0.0.1", 0))
                ports.append(sock.getsockname()[1])
        self.endpoints = {f"n{i + 1}": ("127.0.0.1", ports[i]) for i in range(self.member_count)}
        self.members = [{"id": node, "client": f"{host}:{port}", "peer": f"{host}:{ports[i + self.member_count]}"}
                        for i, (node, (host, port)) in enumerate(self.endpoints.items())]
        self.cluster = "books-" + secrets.token_hex(8)
        self.config = self.root / "cluster.json"
        self.write_config(self.config, self.cluster, self.members)
        self.token = secrets.token_hex(32)
        self.peer_token = secrets.token_hex(32)
        self.secret = self.root / "client-token"
        self.peer_secret = self.root / "peer-token"
        self.secret.write_text(self.token, encoding="ascii")
        self.peer_secret.write_text(self.peer_token, encoding="ascii")
        self.servers = {}
        self.bootstrapped = set()
        self.addCleanup(self.stop_all)

    @staticmethod
    def write_config(path, cluster, members):
        path.write_text(json.dumps({"cluster_id": cluster, "members": members}), encoding="utf-8")

    def command(self, node, config=None, peer_secret=None):
        host, port = self.endpoints[node]
        bootstrap = [] if node in self.bootstrapped else ["--init"]
        return [str(BINARY), "--db", str(self.root / f"{node}.ndb"), *bootstrap, "--bind", f"{host}:{port}",
                "--cluster-config", str(config or self.config), "--node-id", node,
                "--token-file", str(self.secret), "--peer-token-file", str(peer_secret or self.peer_secret)]

    def start(self, node, peer_secret=None, *, extra=()):
        with (self.root / f"{node}.log").open("ab") as log:
            self.servers[node] = subprocess.Popen([*self.command(node, peer_secret=peer_secret), *extra],
                                                  stdout=log, stderr=subprocess.STDOUT)
        self.await_condition(f"{node} to accept authenticated status requests", lambda: self.status(node))
        self.bootstrapped.add(node)

    def stop(self, node):
        process = self.servers.pop(node)
        process.kill()
        process.wait(timeout=10)

    def stop_all(self):
        for node in list(self.servers):
            self.stop(node)

    def await_condition(self, description, probe, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for node, process in self.servers.items():
                if process.poll() is not None:
                    self.fail(f"{node} exited waiting for {description}: " +
                              (self.root / f"{node}.log").read_text(errors="replace"))
            try:
                result = probe()
                if result:
                    return result
            except (OSError, TimeoutError):
                pass
            time.sleep(0.075)
        self.fail(f"timed out waiting for {description}; logs: " +
                  repr({node: (self.root / f"{node}.log").read_text(errors="replace")[-1000:]
                        for node in self.servers}))

    def connect(self, node, timeout=8):
        return Client(*self.endpoints[node], self.token, timeout=timeout)

    def discover(self):
        return Client.connect_leader(self.endpoints.values(), self.token, cluster_id=self.cluster,
                                     timeout=8, discovery_timeout=15)

    def ok(self, db, op, **fields):
        result = db.request(op=op, **fields)
        self.assertTrue(result.get("ok"), (op, result))
        return result

    def status(self, node):
        with self.connect(node, timeout=1) as db:
            status = self.ok(db, "cluster_status")
            self.assertEqual(status["cluster_id"], self.cluster)
            self.assertEqual(status["node_id"], node)
            return status

    def verify_data(self, db, expected):
        for key, value in expected.items():
            self.assertEqual(self.ok(db, "get_latest", key=key)["value"], value, key)


class ReplicationTests(ClusterFixture):
    def test_offline_member_installs_compacted_snapshot_and_replays_tail(self):
        for node in self.endpoints:
            self.start(node)
        with self.discover() as leader:
            primary = self.ok(leader, "cluster_status")["node_id"]
            offline, other = [n for n in self.endpoints if n != primary]
            initial = int(self.ok(leader, "put_commit", key="00", value="dead")["version"])
            self.await_condition("original value on offline member",
                                 lambda: int(self.status(offline)["applied_index"]) >= initial)
            self.stop(offline)
            self.ok(leader, "delete_commit", key="00")
            boundary = initial
            for byte in ("ab", "cd"):
                payload = byte * (240 * 1024)
                for _ in range(19):
                    self.ok(leader, "put_commit", key="01", value=payload)
                self.await_condition("leader to advance the compacted prefix",
                                     lambda: int(self.status(primary)["snapshot_index"]) > boundary)
                boundary = int(self.status(primary)["snapshot_index"])
            tail = int(self.ok(leader, "put_commit", key="02", value="beef")["version"])
            self.assertLess(int(self.status(primary)["snapshot_index"]), tail)
            self.start(offline)
            self.await_condition("snapshot and subsequent entries on returning member",
                                 lambda: int(self.status(offline)["applied_index"]) == tail)
            self.assertGreaterEqual(int(self.status(offline)["installed_snapshot_index"]), boundary)
            self.stop(other)
            self.ok(leader, "put_commit", key="03", value="aa")
        self.stop(primary)
        self.stop(offline)
        self.start(offline)
        self.start(other)
        with self.discover() as restored:
            self.assertEqual(self.ok(restored, "cluster_status")["node_id"], offline)
            self.verify_data(restored, {"00": None, "01": payload, "02": "beef", "03": "aa"})
            self.assertEqual(self.ok(restored, "ping")["version"],
                             self.ok(restored, "cluster_status")["applied_index"])

    @unittest.skipUnless(os.getenv("CATDB_SQL_HOST"), "requires SilverHamu.Sql.Host")
    def test_sql_grants_survive_failover(self):
        viewer_token = secrets.token_hex(32)
        viewer_secret = self.root / "viewer.token"
        viewer_secret.write_text(viewer_token, encoding="ascii")
        extra = ["--sql-host", os.environ["CATDB_SQL_HOST"], "--read-role", "reader",
                 "--read-token-file", str(viewer_secret)]
        for node in self.endpoints:
            self.start(node, extra=extra)
        with self.discover() as admin:
            old = self.ok(admin, "cluster_status")["node_id"]
            self.ok(admin, "sql", sql="CREATE TABLE books(id INT PRIMARY KEY); INSERT INTO books VALUES (7); "
                                      "CREATE VIEW visible AS SELECT id FROM books; GRANT SELECT ON visible TO reader")
        self.stop(old)
        with self.discover() as successor:
            self.assertNotEqual(self.ok(successor, "cluster_status")["node_id"], old)
            self.start(old, extra=extra)
            with Client.connect_leader(self.endpoints.values(), viewer_token, cluster_id=self.cluster) as viewer:
                rows = self.ok(viewer, "sql", sql="SELECT id FROM visible")["results"][0]["Rows"]
                self.assertEqual(rows[0][0]["Text"], "7")
                self.assertEqual(viewer.request(op="get_latest", key="01")["error"], "forbidden")
                self.ok(successor, "sql", sql="REVOKE SELECT ON visible FROM reader")
                self.assertFalse(viewer.request(op="sql", sql="SELECT id FROM visible")["ok"])

    def test_acknowledged_mutations_survive_failover_rejoin_quorum_loss_and_full_restart(self):
        for node in self.endpoints:
            self.start(node)
        expected = {"01": "11", "02": "22", "03": "", "04": None}
        with self.discover() as leader:
            status = self.ok(leader, "cluster_status")
            old = status["node_id"]
            self.ok(leader, "put_commit", key="01", value="11")
            self.ok(leader, "put_commit", key="04", value="ff")
            self.ok(leader, "begin")
            self.ok(leader, "put", key="02", value="22")
            self.ok(leader, "put", key="03", value="")
            self.ok(leader, "commit")
            self.ok(leader, "delete_commit", key="04")
            self.ok(leader, "begin")
            self.ok(leader, "put", key="08", value="88")
            expected["08"] = None
            self.stop(old)

        with self.discover() as successor:
            elected = self.ok(successor, "cluster_status")
            self.assertNotEqual(elected["node_id"], old)
            self.assertGreater(int(elected["term"]), int(status["term"]))
            self.verify_data(successor, expected)
            self.ok(successor, "put_commit", key="05", value="55")
            expected["05"] = "55"
            committed = int(self.ok(successor, "cluster_status")["commit_index"])
            other = next(node for node in self.servers if node != elected["node_id"])
            self.stop(other)
            # A lost acknowledgement is ambiguous. Do not assume this key can never appear later.
            denied = successor.request(op="put_commit", key="06", value="66")
            self.assertFalse(denied["ok"], denied)
            self.assertIn(denied["error"], ("not_leader", "quorum_unavailable", "commit_uncertain"))
        self.start(old)
        self.await_condition("the former leader to apply the missed commit",
                             lambda: int(self.status(old)["applied_index"]) >= committed)
        with self.discover() as restored:
            self.verify_data(restored, expected)
            self.ok(restored, "put_commit", key="07", value="77")
            expected["07"] = "77"

        self.stop_all()
        for node in self.endpoints:
            self.start(node)
        with self.discover() as restarted:
            self.verify_data(restarted, expected)

    def test_transaction_conflict_and_rollback_do_not_replicate_staged_values(self):
        for node in self.endpoints:
            self.start(node)
        with self.discover() as first, self.discover() as second:
            self.ok(first, "put_commit", key="01", value="aa")
            self.ok(first, "begin")
            self.assertEqual(self.ok(first, "get", key="01")["value"], "aa")
            self.ok(second, "put_commit", key="01", value="bb")
            self.assertEqual(self.ok(first, "get", key="01")["value"], "aa")
            self.ok(first, "put", key="02", value="22")
            self.assertEqual(first.request(op="commit").get("error"), "conflict")
            self.ok(second, "begin")
            self.ok(second, "put", key="03", value="33")
            self.ok(second, "rollback")
            self.ok(first, "begin")
            self.ok(first, "lock_keys", keys=["04"])
            self.ok(second, "begin")
            denied = second.request(op="put", key="04", value="44")
            self.assertEqual(denied.get("error"), "conflict", denied)
            self.ok(first, "rollback")
            self.ok(second, "put", key="04", value="44")
            self.ok(second, "commit")
            self.verify_data(second, {"01": "bb", "02": None, "03": None})
            self.stop(self.ok(second, "cluster_status")["node_id"])
        with self.discover() as successor:
            self.verify_data(successor, {"01": "bb", "02": None, "03": None, "04": "44"})

    def test_paused_leader_cannot_commit_an_open_transaction_after_a_new_term(self):
        for node in self.endpoints:
            self.start(node)
        with self.discover() as old_session:
            status = self.ok(old_session, "cluster_status")
            old = status["node_id"]
            self.ok(old_session, "begin")
            self.ok(old_session, "lock_keys", keys=["ac"])
            self.ok(old_session, "put", key="ab", value="01")
            with suspend(self.servers[old]):
                others = [endpoint for node, endpoint in self.endpoints.items() if node != old]
                with Client.connect_leader(others, self.token, cluster_id=self.cluster,
                                           discovery_timeout=15) as successor:
                    elected = self.ok(successor, "cluster_status")
                    self.assertGreater(int(elected["term"]), int(status["term"]))
                    self.ok(successor, "begin")
                    self.ok(successor, "lock_keys", keys=["ac"])
                    self.ok(successor, "put", key="ac", value="02")
                    self.ok(successor, "commit")
            # This is the pre-election TCP session and its staged write from the old term.
            try:
                denied = old_session.request(op="commit")
            except ConnectionError:
                pass
            else:
                self.assertFalse(denied["ok"], denied)
                self.assertIn(denied["error"], ("not_leader", "quorum_unavailable", "commit_uncertain"))
        with self.discover() as leader:
            self.verify_data(leader, {"ab": None, "ac": "02"})

    def test_client_token_cluster_identity_and_peer_secret_are_checked(self):
        wrong = self.root / "wrong-peer-token"
        wrong.write_text(secrets.token_hex(32), encoding="ascii")
        self.start("n1")
        self.start("n2")
        self.start("n3", peer_secret=wrong)
        with self.discover() as leader:
            node = self.ok(leader, "cluster_status")["node_id"]
            self.assertIn(node, ("n1", "n2"))
            self.ok(leader, "put_commit", key="11", value="ab")
            with self.assertRaisesRegex(RuntimeError, "authentication failed"):
                Client(*self.endpoints[node], self.peer_token, timeout=2)
            with self.assertRaisesRegex(ValueError, "different silverhamu cluster"):
                Client.connect_leader([self.endpoints[node]], self.token, cluster_id="another-cluster")
            follower = "n1" if node == "n2" else "n2"
            with self.connect(follower) as db:
                reply = db.request(op="put_commit", key="12", value="bb")
                self.assertEqual(reply.get("error"), "not_leader", reply)
            self.stop(follower)
            denied = leader.request(op="put_commit", key="13", value="cc")
            self.assertFalse(denied["ok"], denied)
            self.assertIn(denied["error"], ("not_leader", "quorum_unavailable", "commit_uncertain"))

    def test_fixed_membership_cannot_be_reinterpreted_on_startup(self):
        invalid = self.root / "invalid.json"
        self.write_config(invalid, self.cluster, [self.members[0], self.members[0], self.members[2]])
        result = subprocess.run(self.command("n1", invalid), capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        command = self.command("n1")
        command[command.index("--node-id") + 1] = "outsider"
        self.assertNotEqual(subprocess.run(command, capture_output=True, timeout=10).returncode, 0)
        for node in self.endpoints:
            self.start(node)
        with self.discover() as db:
            self.ok(db, "put_commit", key="aa", value="bb")
        self.stop_all()
        self.write_config(invalid, self.cluster + "-other", self.members)
        result = subprocess.run(self.command("n1", invalid), capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        data = self.root / "n1.ndb"
        journal = self.root / "n1.ndb.raft"
        for missing in ((data,), (journal,), (data, journal)):
            with self.subTest(missing=[p.name for p in missing]), ExitStack() as restore:
                for path in missing:
                    saved = path.rename(path.with_name(path.name + ".saved"))
                    restore.callback(saved.rename, path)
                refused = subprocess.run(self.command("n1"), capture_output=True, timeout=10)
                self.assertNotEqual(refused.returncode, 0)
                self.assertNotIn(b"ready", refused.stdout)
                for path in missing:
                    self.assertFalse(path.exists(), "a restarting member must not recreate missing state")
        for node in self.endpoints:
            self.start(node)
        with self.discover() as db:
            self.verify_data(db, {"aa": "bb"})


class OnlineMembershipTests(ClusterFixture):
    member_count = 5

    def setUp(self):
        super().setUp()
        self.write_config(self.config, self.cluster, self.members[:3])
        self.admin = secrets.token_hex(32)
        self.admin_file = self.root / "membership.token"
        self.admin_file.write_text(self.admin, encoding="ascii")

    def command(self, node, config=None, peer_secret=None):
        if node in ("n4", "n5"):
            config = self.root / f"{node}.json"
            joining = next(m for m in self.members if m["id"] == node)
            config.write_text(json.dumps({"cluster_id": self.cluster, "members": self.members[:3],
                                          "joining": joining}), encoding="utf-8")
        return super().command(node, config, peer_secret) + ["--cluster-admin-token-file", str(self.admin_file)]

    def change(self, client, members):
        index = self.ok(client, "cluster_status")["configuration_index"]
        return self.ok(client, "cluster_reconfigure", members=members, admin_token=self.admin,
                       expected_configuration_index=index)

    def test_planned_transfer_to_specified_voter_before_old_leader_shutdown(self):
        for node in ("n1", "n2", "n3"):
            self.start(node)
        with self.discover() as old:
            before = self.ok(old, "cluster_status")
            source = before["node_id"]
            target = next(n for n in ("n3", "n2") if n != source)
            request = dict(op="cluster_transfer_leader", target=target, expected_term=before["term"])
            self.assertEqual(old.request(**request)["error"], "forbidden")
            request["admin_token"] = self.admin
            for invalid in (source, "n4"):
                self.assertEqual(old.request(**{**request, "target": invalid})["error"], "invalid_transfer_target")
            self.assertEqual(old.request(**{**request, "expected_term":"0"})["error"], "term_changed")
            other = next(n for n in ("n1","n2","n3") if n not in (source,target))
            self.stop(other)
            self.assertEqual(old.request(**request)["error"], "maintenance_quorum_unavailable")
            self.assertEqual(self.status(source)["term"], before["term"])
            self.start(other)
            self.stop(target)
            version = int(self.ok(old, "put_commit", key="11", value="aa")["version"])
            self.assertEqual(old.request(**request)["error"], "target_not_ready")
            self.ok(old, "put_commit", key="22", value="bb")
            self.start(target)
            transfer = self.ok(old, **request)
            self.assertEqual(transfer["leader_id"], target)
            self.assertGreater(int(transfer["term"]), int(before["term"]))
            self.assertGreater(int(transfer["commit_index"]), version)
            self.assertEqual(self.status(target)["role"], "leader")
            self.stop(source)
        with self.connect(target) as new:
            self.verify_data(new, {"11": "aa", "22": "bb"})
            self.ok(new, "put_commit", key="33", value="cc")
            self.verify_data(new, {"33": "cc"})

    def stable(self, node, members):
        status = self.status(node)
        return (status["membership_phase"] == "stable" and set(status["members"]) == members
                and int(status["applied_index"]) >= int(status["configuration_index"]))

    def test_expand_three_to_five_then_remove_leader_and_write_on_remaining_three(self):
        for node in ("n1", "n2", "n3"):
            self.start(node)
        with self.discover() as first:
            leader = self.ok(first, "cluster_status")["node_id"]
            self.ok(first, "put_commit", key="01", value="aa")
            for _ in range(128):
                self.ok(first, "put_commit", key="04", value="dd")
            self.await_condition("snapshot before expansion", lambda: int(self.status(leader)["snapshot_index"]) > 0)
            self.assertEqual(first.request(op="cluster_reconfigure", members=self.members)["error"], "forbidden")
            with self.connect(leader) as locker:
                self.ok(locker, "begin")
                self.ok(locker, "lock_keys", keys=[b"silverhamu-raft-noop".hex()])
                self.change(first, self.members)
                self.ok(locker, "rollback")
            self.assertEqual(self.status(leader)["membership_phase"], "catch_up")
            self.ok(first, "put_commit", key="02", value="bb")
            for node in ("n4", "n5"):
                self.start(node)
            self.await_condition("all five voters caught up", lambda: all(
                self.stable(n, set(self.endpoints)) for n in self.servers))
            for node in ("n4","n5"):
                self.assertGreater(int(self.status(node)["installed_snapshot_index"]),0)
            second = next(n for n in ("n1", "n2", "n3") if n != leader)
            remaining = set(self.endpoints) - {leader, second}
            self.change(first, [m for m in self.members if m["id"] in remaining])
            self.await_condition("remaining three voters committed", lambda: all(
                self.stable(n, remaining) for n in remaining))
            for removed in (leader, second):
                with self.connect(removed) as old:
                    self.assertFalse(old.request(op="get_latest", key="01")["ok"])
                    self.assertFalse(old.request(op="put_commit", key="03", value="00")["ok"])
                self.stop(removed)
        with self.discover() as current:
            self.assertIn(self.ok(current, "cluster_status")["node_id"], remaining)
            self.verify_data(current, {"01": "aa", "02": "bb", "03": None})
            self.ok(current, "put_commit", key="03", value="cc")
        # The original three voters are now all offline. Only the two new members can elect and read.
        original = next(n for n in remaining if n not in ("n4", "n5"))
        self.stop(original)
        with self.discover() as current:
            self.assertIn(self.ok(current, "cluster_status")["node_id"], ("n4", "n5"))
            self.verify_data(current, {"01": "aa", "02": "bb", "03": "cc"})
            final_index = int(self.ok(current, "cluster_status")["configuration_index"])
            payload = "ee" * (240 * 1024)
            for _ in range(19):
                self.ok(current, "put_commit", key="05", value=payload)
            self.await_condition("snapshot includes final membership", lambda: all(
                int(self.status(n)["snapshot_index"]) >= final_index for n in ("n4", "n5")))
            self.start(original)
            self.await_condition("retained voter installs the new configuration from a peer snapshot",
                                 lambda: int(self.status(original)["installed_snapshot_index"]) >= final_index)
        self.stop_all()
        for node in remaining:
            self.start(node)
        with self.discover() as restarted:
            self.verify_data(restarted, {"01": "aa", "02": "bb", "03": "cc"})
            self.assertEqual(set(self.ok(restarted, "cluster_status")["members"]), remaining)
            self.start(leader)
            self.assertFalse(self.status(leader)["is_voter"])
            with self.connect(leader) as retired:
                self.assertFalse(retired.request(op="put_commit", key="06", value="ff")["ok"])
            self.ok(restarted, "put_commit", key="06", value="de")

    def test_new_leader_can_cancel_unfinished_learner_catch_up(self):
        for node in ("n1", "n2", "n3"):
            self.start(node)
        with self.discover() as old:
            primary=self.ok(old,"cluster_status")["node_id"]
            self.change(old,self.members)
            self.stop(primary)
        with self.discover() as leader:
            self.assertEqual(self.ok(leader,"cluster_status")["membership_phase"],"catch_up")
            stale=leader.request(op="cluster_reconfigure", members=self.members[:3],admin_token=self.admin,
                                 expected_configuration_index="0")
            self.assertEqual(stale["error"],"configuration_changed")
            self.change(leader,self.members[:3])
            self.ok(leader,"put_commit",key="01",value="ff")
            self.verify_data(leader,{"01":"ff"})


class FiveMemberTests(ClusterFixture):
    member_count = 5

    def test_three_of_five_members_can_elect_and_commit_but_two_cannot(self):
        for node in self.endpoints:
            self.start(node)
        with self.discover() as leader:
            original = self.ok(leader, "cluster_status")
            self.ok(leader, "put_commit", key="11", value="11")
        self.stop(original["node_id"])
        self.stop(next(iter(self.servers)))
        with self.discover() as successor:
            elected = self.ok(successor, "cluster_status")
            self.assertGreater(int(elected["term"]), int(original["term"]))
            self.verify_data(successor, {"11": "11"})
            self.ok(successor, "put_commit", key="22", value="22")
            third = next(node for node in self.servers if node != elected["node_id"])
            self.stop(third)
            denied = successor.request(op="put_commit", key="33", value="33")
            self.assertFalse(denied["ok"], denied)
            self.assertIn(denied["error"], ("not_leader", "quorum_unavailable", "commit_uncertain"))
        self.start(third)
        with self.discover() as restored:
            self.verify_data(restored, {"11": "11", "22": "22"})


if __name__ == "__main__":
    unittest.main()
