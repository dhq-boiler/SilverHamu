"""Experimental catdb v1 client. Requests are JSON lines on stdin; values are hex."""
import argparse
import json
import pathlib
import socket
import struct
import sys


class Client:
    def __init__(self, host, port, token, timeout=10):
        self.socket = socket.create_connection((host, port), timeout)
        try:
            response = self.request(op="auth", version=1, token=token)
            if not response.get("ok"):
                raise RuntimeError("catdb authentication failed")
        except BaseException:
            self.close()
            raise

    def receive_exact(self, count):
        chunks = bytearray()
        while len(chunks) < count:
            chunk = self.socket.recv(count - len(chunks))
            if not chunk:
                raise ConnectionError("connection closed; a submitted commit may have completed")
            chunks.extend(chunk)
        return chunks

    def request(self, **request):
        body = json.dumps(request, separators=(",", ":")).encode()
        if len(body) > 1024 * 1024:
            raise ValueError("request exceeds 1 MiB")
        self.socket.sendall(struct.pack("!I", len(body)) + body)
        size = struct.unpack("!I", self.receive_exact(4))[0]
        if not 0 < size <= 2 * 1024 * 1024 + 8192:
            raise ValueError("invalid response size")
        return json.loads(self.receive_exact(size))

    def close(self):
        self.socket.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7400)
    parser.add_argument("--token-file", type=pathlib.Path, required=True)
    args = parser.parse_args()
    token = args.token_file.read_text(encoding="utf-8").rstrip("\r\n")
    with Client(args.host, args.port, token) as client:
        for line in sys.stdin:
            if not line.strip():
                continue
            response = client.request(**json.loads(line))
            print(json.dumps(response, ensure_ascii=False), flush=True)
            if not response.get("ok"):
                return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"catdb-client: {error}", file=sys.stderr)
        sys.exit(1)
