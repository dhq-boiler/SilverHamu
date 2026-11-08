"""Experimental catdb v1 client. Requests are JSON lines on stdin; values are hex."""
import argparse
import getpass
import json
import pathlib
import socket
import ssl
import struct
import sys


class Client:
    def __init__(self, host, port, token=None, timeout=10, *, username=None, password=None,
                 database="main", cafile=None):
        if (token is None) == (username is None):
            raise ValueError("choose token or account authentication")
        if token is not None and (password is not None or cafile is not None or database != "main"):
            raise ValueError("TLS account options cannot be used with a token")
        self.socket = socket.create_connection((host, port), timeout)
        try:
            if username is not None:
                context = ssl.create_default_context(cafile=cafile)
                context.minimum_version = ssl.TLSVersion.TLSv1_3
                self.socket = context.wrap_socket(self.socket, server_hostname=host)
                response = self.request(op="auth", version=2, database=database,
                                        username=username, password=password)
            else:
                response = self.request(op="auth", version=1, token=token)
            if not response.get("ok"):
                raise RuntimeError("catdb authentication failed")
            self.account_id = response.get("account_id")
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

    @staticmethod
    def _encode(request):
        body = json.dumps(request, separators=(",", ":")).encode()
        if len(body) > 1024 * 1024:
            raise ValueError("request exceeds 1 MiB")
        return body

    def request(self, **request):
        return self._exchange(self._encode(request))

    def _exchange(self, body):
        self.socket.sendall(struct.pack("!I", len(body)) + body)
        size = struct.unpack("!I", self.receive_exact(4))[0]
        if not 0 < size <= 2 * 1024 * 1024 + 8192:
            raise ValueError("invalid response size")
        return json.loads(self.receive_exact(size).decode("utf-8"))

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
    auth = parser.add_mutually_exclusive_group(required=True)
    auth.add_argument("--token-file", type=pathlib.Path)
    auth.add_argument("--user")
    parser.add_argument("--password-file", type=pathlib.Path)
    parser.add_argument("--ca-file")
    parser.add_argument("--database", default="main")
    parser.add_argument("--create-user")
    parser.add_argument("--role", choices=("reader","writer","admin"))
    args = parser.parse_args()
    if args.password_file and not args.user:
        parser.error("--password-file requires --user")
    if bool(args.create_user) != bool(args.role) or (args.create_user and not args.user):
        parser.error("--create-user and --role require account authentication and each other")
    token = args.token_file.read_text(encoding="utf-8").rstrip("\r\n") if args.token_file else None
    password = None
    if args.user:
        if args.password_file:
            with args.password_file.open("rb") as source:
                secret = source.read(1027)
            if len(secret) > 1026:
                raise ValueError("password file too large")
            password = secret.decode("utf-8")
            if password.endswith("\n"):
                password = password[:-1].removesuffix("\r")
        else:
            password = getpass.getpass("Account password: ")
    with Client(args.host, args.port, token, username=args.user, password=password,
                database=args.database, cafile=args.ca_file) as client:
        if args.create_user:
            new_password = getpass.getpass("New user's password: ")
            if new_password != getpass.getpass("Confirm new password: "):
                raise ValueError("passwords differ")
            response = client.request(op="create_user", username=args.create_user,
                                      password=new_password, role=args.role)
            print(json.dumps(response))
            return 0 if response.get("ok") else 1
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
