# Replication and automatic failover

The token-authenticated service can host one database on three or five voting nodes. Each node owns a local database and a durable Raft journal. A successful commit means a majority has durably recorded the transaction and the leader has applied it. With three nodes, two must be available. With five nodes, three must be available. After a leader failure, the surviving majority elects a successor and retains acknowledged writes. The protocol uses Raft's election, log-matching and current-term commit rules. [Raft paper, sections 5 and 8](https://raft.github.io/raft.pdf)

## Start three nodes

Build the service with `cargo build --release --locked --features service --bin catdb-server`. The example runs three processes on one host. To tolerate loss of a host, place each member on a separate machine with its own durable disk.

Save this configuration as `cluster.json` and give every member the same file. Use a unique cluster ID and stable numeric client and peer addresses.

```json
{
  "cluster_id": "books",
  "members": [
    {"id": "n1", "client": "127.0.0.1:7401", "peer": "127.0.0.1:7501"},
    {"id": "n2", "client": "127.0.0.1:7402", "peer": "127.0.0.1:7502"},
    {"id": "n3", "client": "127.0.0.1:7403", "peer": "127.0.0.1:7503"}
  ]
}
```

Create two independently generated secrets, each 32–256 UTF-8 bytes, and distribute them over a secure channel. Give clients access to `client.token` and restrict `peer.token` to the servers. Peers authenticate requests and replies using HMAC-SHA256 and fresh connection challenges. Remote deployment requires `--allow-remote` and an encrypted, access-controlled network or tunnel for **both** ports. Cluster traffic contains database contents.

Run one command per terminal. On Windows the executable is `target\release\catdb-server.exe`.

```shell
catdb-server --cluster-config cluster.json --node-id n1 --db n1.ndb --init --token-file client.token --peer-token-file peer.token
catdb-server --cluster-config cluster.json --node-id n2 --db n2.ndb --init --token-file client.token --peer-token-file peer.token
catdb-server --cluster-config cluster.json --node-id n3 --db n3.ndb --init --token-file client.token --peer-token-file peer.token
```

The service binds the client address for its node. To serve SQL, configure the same SQL host version and role-token policy on every node with `--sql-host` and the usual role options. Schema, grants and row changes share the same replicated transaction log. TLS accounts, attached databases and the benchmark-only batch protocol are outside the cluster service. Embedded databases and Windows Owner retain their existing single-node behavior.

## Connect and recover from a disconnect

The Python client accepts a trusted list of **client** addresses. It checks the cluster ID and confirms that the reported leader can reach a majority before returning the connection. [Python client example](../clients/python/README.md)

```python
from pathlib import Path
from catdb_client import Client

seeds = [("127.0.0.1", port) for port in (7401, 7402, 7403)]
token = Path("client.token").read_text().rstrip("\r\n")
with Client.connect_leader(seeds, token, cluster_id="books") as db:
    db.request(op="put_commit", key="01", value="abcd")
    print(db.request(op="get_latest", key="01")["value"])
```

| Observation | Application action |
|---|---|
| `not_leader` or `quorum_unavailable` | Rediscover the leader. An in-progress transaction on the old connection must be restarted. |
| `commit_uncertain`, or disconnect after sending a write | The transaction may have committed. Reconnect and check the business operation's identifier or its result before issuing another write. The client never replays a write automatically. |
| Successful `cluster_status` | Inspect `node_id`, `role`, `leader_id`, `term`, `commit_index` and `applied_index`. Indices and terms are decimal strings. Status describes this node's local knowledge. A successful `ping` also requires fresh contact with a majority. |

Each read checks current leadership. An isolated former leader may still report its old role, but cannot serve data or acknowledge a new commit. An explicit transaction keeps its snapshot and row locks on the leader that opened it. A change of term invalidates that transaction. The election barrier consumes a log position, so database versions can advance during an election.

## Restart and capacity

Use equivalent resource budgets on all members. A member under memory pressure may temporarily report an applied index below its commit index. It retries application as snapshots end and capacity becomes available. A leader in that state refuses data operations until it has caught up. If the live data exceeds a node's configured budget, increase the budget and restart that node with its original files.

Restart a stopped member with the same ID, topology, credentials and database path, omitting `--init`. It replays committed entries and catches up from its peers. If a majority stops, restore enough original members to form a majority. Keep each application file and its `.raft` journal together. The journal includes durable votes as well as data. Reinitializing a previously voting identity or rolling it back to an older disk image can invalidate the voting guarantees.

Membership and endpoints are fixed. Bootstrap requires fresh paths on all members. Existing standalone databases must be imported through the leader. Offline copying, restoring individual files, and editing replica files outside the service are unsupported. There is no online member replacement, snapshot transfer or log compaction.

A single transaction's encoded write set is limited to 1 MiB, including hex and JSON overhead. The in-memory log and journal retain up to 65,536 entries or 128 MiB of accounted log data, with 1,024 entries and 64 KiB reserved for elections. Capacity exhaustion returns `limit` for new writes. Plan a logical export and import into a fresh cluster before reaching capacity. This bounded implementation is experimental.
