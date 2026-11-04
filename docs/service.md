# Experimental service v1

The service is opt-in (`--features service`) and uses the same database implementation as embedded clients. A process exclusively owns a transaction store; multiple connections share that owner. This is not a server for immutable snapshot files or application-specific archives.

## Configuration

```text
catdb-server --db PATH --token-file PATH [--init]
  [--bind 127.0.0.1:7400] [--allow-remote]
  [--max-connections 32]
  [--request-timeout-secs 30]
  [--transaction-timeout-secs 60]
```

Parent directories must already exist. Without `--init`, a missing database is an error. Existing corrupt files are rejected, not replaced. Non-loopback binding requires explicit `--allow-remote`. Docker uses it inside the container; Compose limits host publication to loopback.

The token file must contain 32–256 UTF-8 bytes, with optional trailing CR/LF. All clients sharing the token have full read/write/checkpoint access. Protect this file. There is no role separation or built-in transport encryption.

```text
catdb-server --healthcheck --token-file PATH [--bind 127.0.0.1:7400]
```

Health checks authenticate and query database state. The Docker healthcheck assumes the default token mount and port. Override it if changing those settings.

## Wire format

Every message is a four-byte big-endian unsigned length followed by that many UTF-8 JSON bytes. The first request is `{"op":"auth","version":1,"token":"..."}`. Successful authentication returns `{"ok":true,"protocol":1}`. Send one request at a time on a connection. The Python client implements the framing.

| Operation | Fields | Success result (in addition to `ok: true`) |
|---|---|---|
| ping | none | version: decimal string |
| begin | none | version: pinned snapshot version |
| get | key | value: hexadecimal string or null |
| put | key, value | none |
| delete | key | none |
| scan | start, optional end/after/rows/bytes/visits | rows: key/value pairs, next_after, visited |
| commit | none | version: committed version |
| rollback | none | none |
| checkpoint | no active transaction on this connection | version, records |

Keys and values are hexadecimal. A missing value is `null`; an empty value is `""`. `get`, `put`, `delete`, `scan`, `commit`, and `rollback` require `begin`. Repeated `begin` is rejected without discarding the current transaction. Commit consumes its transaction even on conflict or persistence failure.

Ranges are `start <= key < end`; omit `end` or use null for no upper bound. Empty `start` begins at the first key. `after` is exclusive. Continue using `next_after` on the same transaction with the same range. An empty page can still have a continuation key. Writes staged in that transaction affect subsequent reads.

Read-only transactions can finish at their original snapshot. A writing transaction validates write, read, and range dependencies. Successful commit replies are sent only after durable log synchronization. Checkpoints preserve pinned old readers.

## Bounds

| Resource | Default / maximum |
|---|---|
| Connections | 32; configurable 1–256 |
| Request JSON bytes | 1 MiB |
| Response JSON bytes | 2 MiB + 8 KiB |
| Key bytes | 1–1024; only scan start permits empty |
| Value written | 0–256 KiB |
| Value returned by get | at most 512 KiB; larger preexisting values return limit |
| Scan rows | 100 / 4096 |
| Scan key/value payload | 64 KiB / 512 KiB |
| Scan visited keys | 4096 / 65536 |
| Frame read/write deadline | 30 seconds; configurable 1–3600 |
| Authentication deadline | shorter of 5 seconds and frame deadline |
| Transaction lifetime | 60 seconds; configurable 1–3600 |

Frame deadlines cover the entire frame and are not extended by partial sends. Transaction expiry is fixed at begin, not extended by requests. Deadlines apply at request boundaries and while waiting for network data; they do not cancel an in-progress storage synchronization.

Engine defaults bound staged changes to 32 MiB, dependencies to 16 MiB, retained data to 128 MiB, and processing buffers to 16 MiB. These are not a strict RSS budget: network buffers, JSON allocations, OS resources, and recovery allocations also consume memory. The engine currently loads database contents into memory.

## Errors and recovery

Errors normally return `{"ok":false,"error":"code"}`. Codes include `invalid_request`, `limit`, `resource_exhausted`, `conflict`, `storage_error`, `commit_uncertain`, `checkpoint_uncertain`, `reopen_required`, and `unauthorized`.

Authentication failure, malformed frames/JSON, and expired deadlines close the connection. Overloaded connections are immediately closed rather than queued. Disconnects discard staged, uncommitted transactions.

**A lost commit response is not evidence of rollback.** The commit may have completed. Reconnect and inspect application-level operation identifiers before retrying. Automatic deduplication is not implemented. Persistence uncertainty can require restarting the service to reopen and recover the store.

The WAL has a 1 GiB limit. Checkpoint explicitly before reaching it; there is no automatic checkpoint scheduler. A full transaction store or exhausted budget is an error, not permission to drop committed data.

SIGTERM/SIGINT stop new acceptance and close idle clients, discard uncommitted changes, and wait for active storage operations. If storage stalls, graceful shutdown can take longer than the requested deadline. Docker's forced termination follows the normal crash-recovery path. Independent backups are still required.

## Container operations

The Docker image uses a multi-stage Rust 1.89/Debian build and runs as UID/GID 10001. Persist the whole `/data` directory; it includes ownership and checkpoint-related files, not just one database file. Mount the token at `/run/secrets/catdb_token` read-only. Do not mount one database directory into multiple active owners.

Compose adds a 512 MiB memory cap, 128-process limit, dropped capabilities, read-only root filesystem, and no-new-privileges. Those settings are deployment starting points, not benchmark results or production guarantees. Named volumes survive `docker compose down`; deleting volumes destroys the data.

Tests cover non-root operation, authenticated health checks, checkpoint failure when directory writes are denied, graceful shutdown with an open transaction, force-kill, and restoring acknowledged values in a recreated container. They do not simulate actual power failure or hardware lying about flush completion.

## Commit grouping

The `service` feature enables group-commit handoff. A coordinator keeps arrivals
queued while the preceding group synchronizes its log, then collects the next
group before acquiring the writer lock. Success still follows log synchronization
and committed-state publication; conflict checks and the file format are unchanged.
The queue is bounded at 32 transactions and 4 MiB of staged write bytes. Requests
that do not fit use the existing single-commit path rather than exceeding those
bounds. The existing 200-microsecond collection budget includes time already spent
queued, and four queued writes wake it early.

See the [paired service measurements](../benchmarks/service-update/README.md) for
reproduction and the distinction between normal and diagnostic builds.
