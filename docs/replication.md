# Replication and automatic failover

The token-authenticated service can host one database on three or five voting nodes. Each node owns a local database and a durable Raft journal. A successful commit means a majority has durably recorded the transaction and the leader has applied it. With three nodes, two must be available. With five nodes, three must be available. After a leader failure, the surviving majority elects a successor and retains acknowledged writes. The protocol uses Raft's election, log-matching and current-term commit rules. [Raft paper, sections 5 and 8](https://raft.github.io/raft.pdf)

## Start three nodes

Build the service with `cargo build --release --locked --features service --bin silverhamu-server`. The example runs three processes on one host. To tolerate loss of a host, place each member on a separate machine with its own durable disk.

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

Run one command per terminal. On Windows the executable is `target\release\silverhamu-server.exe`.

```shell
silverhamu-server --cluster-config cluster.json --node-id n1 --db n1.ndb --init --token-file client.token --peer-token-file peer.token
silverhamu-server --cluster-config cluster.json --node-id n2 --db n2.ndb --init --token-file client.token --peer-token-file peer.token
silverhamu-server --cluster-config cluster.json --node-id n3 --db n3.ndb --init --token-file client.token --peer-token-file peer.token
```

The service binds the client address for its node. To serve SQL, configure the same SQL host version and role-token policy on every node with `--sql-host` and the usual role options. Schema, grants and row changes share the same replicated transaction log. TLS accounts, attached databases and the benchmark-only batch protocol are outside the cluster service. Embedded databases and Windows Owner retain their existing single-node behavior.

## Connect and recover from a disconnect

The Python client accepts a trusted list of **client** addresses. It checks the cluster ID and confirms that the reported leader can reach a majority before returning the connection. [Python client example](../clients/python/README.md)

```python
from pathlib import Path
from silverhamu_client import Client

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

Use the original bootstrap configuration when restarting a member. Its journal and snapshots record subsequent changes to the voting group.

Use equivalent resource budgets on all members. A member under memory pressure may temporarily report an applied index below its commit index. It retries application as snapshots end and capacity becomes available. A leader in that state refuses data operations until it has caught up. If the live data exceeds a node's configured budget, increase the budget and restart that node with its original files.

Restart a stopped member with the same ID, topology, credentials and database path, omitting `--init`. It replays committed entries and catches up from its peers. If a majority stops, restore enough original members to form a majority. Keep each application file and its `.raft` journal together. The journal includes durable votes as well as data. Reinitializing a previously voting identity or rolling it back to an older disk image can invalidate the voting guarantees.

Bootstrap requires fresh paths on all members. Existing standalone databases must be imported through the leader. Offline copying, restoring individual files, and editing replica files outside the service are unsupported.

A single transaction's encoded write set is limited to 1 MiB, including hex and JSON overhead. Each member snapshots its applied state once the committed suffix reaches 128 entries or 8 MiB of accounted log data. The journal publishes the snapshot and its last included index and term before removing the covered entries. Existing journals without snapshots start at index zero. Upgrade all voters together before resuming writes. Older binaries cannot read compacted journals or accept snapshot RPCs.

A returning member whose next entry has been compacted receives the snapshot in authenticated 128 KiB chunks. It verifies the SHA-256 digest, publishes the image durably, installs the key-value state and replays the remaining log. A restart during transfer restarts the download. A restart during installation resumes recovery from the durable image before serving data. In `cluster_status`, `snapshot_index` is the compacted prefix boundary, `log_entries` counts retained entries, `installed_snapshot_index` identifies the last image installed from a peer, and `applied_index` shows catch-up progress.

The encoded live image is limited to 128 MiB and is buffered in memory. Capture uses a pinned read transaction. Publication and installation can temporarily delay consensus processing on that member. If compaction cannot run, the retained log still has its 65,536-entry and 128 MiB limits, including reserved election space.

## Change membership online

Upgrade the voters to a membership-aware binary before the first change. Give each server the same `--cluster-admin-token-file admin.token` setting to enable administration after a failover. Use a separate 32–256 byte secret and keep it with the operators. Application connections use the existing client token. Membership changes additionally require the admin token. Existing clusters can provision the setting by restarting one voter at a time while the others maintain a majority. A node that has stored a membership change requires a membership-aware binary on restart.

To grow the three-node `books` example to five, give each new server a copy of the **original** `cluster.json` with an additional top-level `joining` object describing just that server. For instance, generate both files and the five-member target with Python:

```python
import json
from pathlib import Path

bootstrap = json.loads(Path("cluster.json").read_text())
target = list(bootstrap["members"])
for i in (4, 5):
    joining = {"id": f"n{i}", "client": f"127.0.0.1:740{i}", "peer": f"127.0.0.1:750{i}"}
    Path(f"n{i}.json").write_text(json.dumps({**bootstrap, "joining": joining}))
    target.append(joining)
```

Start the two new processes on fresh paths with the cluster's client, peer and administration credentials and the same SQL host and role policy as the current voters.

```shell
silverhamu-server --cluster-config n4.json --node-id n4 --db n4.ndb --init --token-file client.token --peer-token-file peer.token --cluster-admin-token-file admin.token
silverhamu-server --cluster-config n5.json --node-id n5 --db n5.ndb --init --token-file client.token --peer-token-file peer.token --cluster-admin-token-file admin.token
```

Send the full desired set of three or five endpoints to the leader. Existing members retain their IDs and addresses. The expected configuration index protects a later change from an obsolete administration request.

```python
from silverhamu_client import Client

seeds = [("127.0.0.1", port) for port in range(7401, 7406)]
client_token = Path("client.token").read_text().rstrip("\r\n")
admin_token = Path("admin.token").read_text().rstrip("\r\n")

def change(members):
    with Client.connect_leader(seeds, client_token, cluster_id="books") as db:
        status = db.request(op="cluster_status")
        return db.request(op="cluster_reconfigure", members=members, admin_token=admin_token,
                          expected_configuration_index=status["configuration_index"])

change(target)
```

A successful `accepted` response records the requested transition. Poll `cluster_status` on the leader for completion. If the connection drops or the index changes, rediscover the leader and inspect the current target before submitting another request.

| Membership phase | How the cluster operates |
|---|---|
| `catch_up` | Original voters continue serving reads and writes. New members receive snapshots or log entries. Each must apply at least the entry that requested its addition. If a learner cannot join, submit the original set with the current configuration index to cancel while still in this phase. |
| `joint` | Elections, read confirmation and commit each require separate majorities of the old and target sets. The leader finishes the transition automatically once the joint configuration is committed. [Raft paper, section 6](https://raft.github.io/raft.pdf) |
| `stable` | Check that `members` equals the target and `applied_index >= configuration_index` using integer comparison. The target voters now form the quorum. A leader excluded from this set steps down when the final configuration commits. |

To shrink back to three, call `change([m for m in target if m["id"] in keep])` with the three IDs to retain in `keep`. Keep the old and target majorities available through the transition. Confirm the stable configuration on the surviving leader, update clients' trusted seed addresses, then stop the removed processes. An offline removed member may still report its previous local configuration. Keep its database and journal together if the identity will be used again. Active transactions on a departing leader must be reopened after the election.

Membership controls voting and replication. A retired machine still holds its local data and any shared credentials provisioned on it. Decommission it through the same access-control and storage procedures used for other credential-bearing servers.

## Transfer leadership before maintenance

Send `cluster_transfer_leader` to the current leader using the cluster admin token. Choose another voter in the stable configuration. The request takes the leader's observed term so an obsolete maintenance request cannot trigger a later election.

```python
with Client.connect_leader(seeds, client_token, cluster_id="books") as db:
    status = db.request(op="cluster_status")
    departing = status["node_id"]
    result = db.request(op="cluster_transfer_leader", target="n2",
                        expected_term=status["term"], admin_token=admin_token)
    print(departing, result["leader_id"], result["term"], result["commit_index"])
```

The source serializes data operations while the target catches up. It checks for a majority excluding itself, then asks the target to start a normal election. Success means the source observed the specified successor in a newer term and the successor confirmed a fresh quorum excluding the source, with its current-term commit applied. You can then stop the departing server and reconnect to the successor. The voting configuration stays the same. Transactions opened on the old leader must be restarted.

If the target cannot catch up (`target_not_ready`) or stopping the source would leave too few reachable voters (`maintenance_quorum_unavailable`), the source returns before triggering the election. If the reply is lost or returns `transfer_uncertain`, keep the source running and verify the leader and surviving quorum before maintenance. Allow a client timeout above the five-second handoff window.
