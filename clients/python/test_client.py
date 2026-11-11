import json
import socket
import struct
import threading
import time
import unittest
from unittest.mock import patch

from silverhamu_client import Client


def frame(data):
    payload = json.dumps(data).encode("utf-8")
    return struct.pack("!I", len(payload)) + payload


class ResponsesTest(unittest.TestCase):
    def test_leader_discovery_stops_at_deadline_when_all_seeds_are_unavailable(self):
        with patch("silverhamu_client._wire.socket.create_connection", side_effect=ConnectionRefusedError):
            started = time.monotonic()
            with self.assertRaises(TimeoutError):
                Client.connect_leader([("unused", 7400)], "token", cluster_id="books", discovery_timeout=0.1)
            self.assertLess(time.monotonic() - started, 2)

    def test_discovery_deadline_covers_partial_status_frames(self):
        local, peer = socket.socketpair()
        self.addCleanup(local.close)
        self.addCleanup(peer.close)
        peer.sendall(frame({"ok": True, "protocol": 1}))
        stopped = threading.Event()

        def trickle():
            try:
                for byte in frame({"ok": True, "cluster_id": "books", "node_id": "n1", "role": "leader", "leader_id": "n1"}):
                    if stopped.wait(0.035):
                        break
                    peer.sendall(bytes([byte]))
            except OSError:
                pass

        worker = threading.Thread(target=trickle)
        worker.start()
        try:
            with patch("silverhamu_client._wire.socket.create_connection", return_value=local):
                started = time.monotonic()
                with self.assertRaises(TimeoutError):
                    Client.connect_leader([("slow.example", 7400)], "token", cluster_id="books", discovery_timeout=0.15)
                self.assertLess(time.monotonic() - started, 2)
        finally:
            stopped.set()
            worker.join(timeout=2)

    def test_discovery_authenticates_only_configured_nodes_and_returns_the_leader_connection(self):
        def member(node, role, ping=None):
            client, peer = socket.socketpair()
            self.addCleanup(client.close)
            self.addCleanup(peer.close)
            peer.sendall(frame({"ok": True, "protocol": 1}) + frame({
                "ok": True, "cluster_id": "books", "node_id": node, "role": role, "leader_id": node if role == "leader" else "n2",
                "leader_address": "unconfigured.example:7000",
            }) + (frame(ping) if ping else b""))
            return client, peer

        follower, _ = member("n1", "follower")
        isolated, _ = member("n3", "leader", {"ok": False, "error": "quorum_unavailable"})
        leader, peer = member("n2", "leader", {"ok": True, "version": "7"})
        seeds = [("first.example", 7401), ("isolated.example", 7403), ("second.example", 7402)]
        with patch("silverhamu_client._wire.socket.create_connection", side_effect=[follower, isolated, leader]) as connect:
            with Client.connect_leader(seeds, "token", cluster_id="books", timeout=3) as db:
                self.assertIs(db.socket, leader)
                self.assertEqual(db.socket.gettimeout(), 3)
                peer.sendall(frame({"ok": True, "value": "61"}))
                self.assertEqual(db.get_latest(b"k"), b"a")
            self.assertEqual([call.args[0] for call in connect.call_args_list], seeds)
            self.assertEqual(follower.fileno(), -1)
            self.assertEqual(isolated.fileno(), -1)

    def connected(self):
        local, peer = socket.socketpair()
        self.addCleanup(peer.close)
        peer.sendall(frame({"ok": True, "protocol": 1}))
        with patch("silverhamu_client._wire.socket.create_connection", return_value=local):
            client = Client("unused", 7400, "test")
        self.addCleanup(client.close)
        return client, peer

    def test_corrupt_value_or_frame_discards_connection(self):
        for reply in [frame({"ok": True, "value": "zz"}), frame({"ok": True}),
                      struct.pack("!I", 4_000_000), struct.pack("!I", 30) + b"{"]:
            with self.subTest(reply=reply):
                client, peer = self.connected()
                peer.sendall(reply)
                peer.shutdown(socket.SHUT_WR)
                with self.assertRaises(ConnectionError):
                    client.get_latest(b"k")
                with self.assertRaisesRegex(ConnectionError, "closed"):
                    client.ping()

    def test_local_size_rejection_leaves_connection_usable(self):
        client, peer = self.connected()
        with self.assertRaises(ValueError):
            client.put_commit(b"k", bytes(1024 * 1024))
        peer.sendall(frame({"ok": True, "version": "18446744073709551615"}))
        self.assertEqual(client.ping(), "18446744073709551615")


if __name__ == "__main__":
    unittest.main()
