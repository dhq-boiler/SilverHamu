"""Synchronous client for the catdb service."""

from binascii import unhexlify
from threading import RLock

from ._wire import Client as _WireClient

__all__ = ["Client", "CatdbError"]


class CatdbError(Exception):
    """Server rejection. The code is the error identifier from the wire."""

    def __init__(self, code):
        self.code = code
        super().__init__(code)


def _hex(value):
    if not isinstance(value, (bytes, bytearray, memoryview)):
        raise TypeError("expected a bytes-like key or value")
    return value.hex()


class Client(_WireClient):
    """One connection and transaction context. Use a separate client per transaction."""

    def __init__(self, host, port, token=None, timeout=10, *, username=None, password=None,
                 database="main", cafile=None):
        if username is not None and password is None:
            raise ValueError("account authentication requires a password")
        self._lock = RLock()
        self._closed = False
        super().__init__(host, port, token, timeout, username=username, password=password,
                         database=database, cafile=cafile)

    def request(self, **fields):
        """Send a wire operation. Versions in responses are decimal strings."""
        body = self._encode(fields)
        with self._lock:
            if self._closed:
                raise ConnectionError("catdb client is closed")
            try:
                response = self._exchange(body)
                if not isinstance(response, dict) or type(response.get("ok")) is not bool:
                    raise ValueError("invalid response envelope")
                if not response["ok"]:
                    if not isinstance(response.get("error"), str):
                        raise ValueError("invalid error response")
                    if response["error"] == "unauthorized":
                        self.close()
                    raise CatdbError(response["error"])
                if fields.get("op") == "auth" and response.get("protocol") != fields["version"]:
                    raise ValueError("unexpected authentication protocol")
                if fields.get("op") in {"ping", "begin", "begin_read", "commit", "put_commit", "delete_commit"}:
                    version = response.get("version")
                    if not isinstance(version, str) or not version.isascii() or not version.isdecimal():
                        raise ValueError("invalid catdb version")
                return response
            except (OSError, ValueError) as error:
                self.close()
                raise ConnectionError("catdb connection discarded; a submitted write may have committed") from error

    def close(self):
        with self._lock:
            if not self._closed:
                self._closed = True
                super().close()

    def _get(self, op, key):
        with self._lock:
            response = self.request(op=op, key=_hex(key))
            try:
                value = response["value"]
                return None if value is None else unhexlify(value)
            except (KeyError, TypeError, ValueError) as error:
                self.close()
                raise ConnectionError("invalid catdb value; connection discarded") from error

    def ping(self):
        return self.request(op="ping")["version"]

    def get_latest(self, key):
        return self._get("get_latest", key)

    def put_commit(self, key, value):
        return self.request(op="put_commit", key=_hex(key), value=_hex(value))["version"]

    def delete_commit(self, key):
        return self.request(op="delete_commit", key=_hex(key))["version"]

    def begin(self, *, read_only=False):
        return self.request(op="begin_read" if read_only else "begin")["version"]

    def get(self, key):
        return self._get("get", key)

    def put(self, key, value):
        self.request(op="put", key=_hex(key), value=_hex(value))

    def delete(self, key):
        self.request(op="delete", key=_hex(key))

    def commit(self):
        return self.request(op="commit")["version"]

    def rollback(self):
        self.request(op="rollback")
