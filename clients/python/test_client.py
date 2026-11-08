import json
import socket
import struct
import unittest
from unittest.mock import patch

from catdb_client import Client


def frame(data):
    payload = json.dumps(data).encode("utf-8")
    return struct.pack("!I", len(payload)) + payload


class ResponsesTest(unittest.TestCase):
    def connected(self):
        local, peer = socket.socketpair()
        self.addCleanup(peer.close)
        peer.sendall(frame({"ok": True, "protocol": 1}))
        with patch("catdb_client._wire.socket.create_connection", return_value=local):
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
