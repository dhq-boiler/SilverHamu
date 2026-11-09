# catdb for Python

Install the package in a Python 3.10 or newer environment. The client uses the Python standard library at runtime.

```sh
python -m pip install catdb-service-client==0.1.0
```

Connect to a running catdb service and set `CATDB_TOKEN` to its shared token. [Starting a server](https://github.com/dhq-boiler/catdb#quick-start)

```python
import os
from catdb_client import Client

with Client("127.0.0.1", 7400, os.environ["CATDB_TOKEN"]) as db:
    version = db.put_commit(b"hello", b"world")
    assert db.get_latest(b"hello") == b"world"
```

For TLS account authentication, pass `username=`, `password=` and optionally `database=` and `cafile=` to the constructor. The hostname must match the certificate. A missing key returns `None`. An empty stored value returns `b""`. Commit versions are decimal strings.

Use `begin()`, `get()`, `put()`, `delete()`, `commit()` and `rollback()` for a transaction. Use `begin(read_only=True)` for a pinned snapshot. The `request(op=..., **fields)` method also supports the wire operations, including paginated scans. Server rejections raise `CatdbError` with a `code` attribute. After a lost reply, open a new client and inspect the result before retrying a write.

For a replicated service, supply its cluster ID, client token and the configured **client** endpoints. The helper authenticates to these addresses and probes for an elected leader.

```python
seeds = [("127.0.0.1", 7401), ("127.0.0.1", 7402), ("127.0.0.1", 7403)]

def connect():
    return Client.connect_leader(seeds, os.environ["CATDB_TOKEN"],
                                 cluster_id="books", discovery_timeout=10)

with connect() as db:
    value = db.get_latest(b"book/42")
```

Following an election or connection failure, call `connect()` again. A new connection has a new transaction context. Discovery verifies the cluster ID and confirms the reported leader can reach a quorum with a read-only ping. If a write reply is lost or reports `commit_uncertain`, inspect the application's committed state on the new leader before deciding whether to resubmit it.
