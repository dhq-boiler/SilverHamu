# Experimental catdb service

For individual logins and database-scoped permissions, use [TLS service accounts](accounts.md). The shared-token configuration below describes the v1 listener.

The service is opt-in (`--features service`) and uses the same database implementation as embedded clients. A process exclusively owns a transaction store; multiple connections share that owner. This is not a server for immutable snapshot files or application-specific archives.

## Configuration

```text
catdb-server --db PATH --token-file PATH [--init]
  [--bind 127.0.0.1:7400] [--allow-remote]
  [--max-connections 32]
  [--request-timeout-secs 30]
  [--transaction-timeout-secs 60]
  [--max-retained-bytes 134217728]
  [--max-staged-bytes 33554432]
  [--max-dependency-bytes 16777216]
  [--max-buffer-bytes 16777216]
```

Parent directories must already exist. Without `--init`, a missing database is an error. Existing corrupt files are rejected, not replaced. Non-loopback binding requires explicit `--allow-remote`. Docker uses it inside the container; Compose limits host publication to loopback.

The token file must contain 32–256 UTF-8 bytes, with optional trailing CR/LF. All clients sharing the token have full read/write/checkpoint access. Protect this file. The v1 token listener has no role separation or built-in transport encryption.

```text
catdb-server --healthcheck --token-file PATH [--bind 127.0.0.1:7400]
```

Health checks authenticate and query database state. The Docker healthcheck assumes the default token mount and port. Override it if changing those settings.

## Wire format

Every message is a four-byte big-endian unsigned length followed by that many UTF-8 JSON bytes. The first request is `{"op":"auth","version":1,"token":"..."}`. Successful authentication returns `{"ok":true,"protocol":1}`. Send one request at a time on a connection. The [client libraries](clients.md) handle framing, authentication and byte encoding.

| Operation | Fields | Success result (in addition to `ok: true`) |
|---|---|---|
| ping | none | version: decimal string |
| begin | none | version: pinned snapshot version |
| begin_read | none | version: pinned read-only snapshot version |
| get | key | value: hexadecimal string or null |
| lock_keys | keys: array of hexadecimal keys, in a read/write transaction | Reserves all keys atomically, up to 4,096 per transaction. A competing lock or write reports `conflict`. |
| put | key, value | none |
| delete | key | none |
| scan | start, optional end/after/rows/bytes/visits | rows: key/value pairs, next_after, visited |
| commit | none | version: committed version |
| rollback | none | none |
| checkpoint | no active transaction on this connection | version, records |

Keys and values are hexadecimal. A missing value is `null`; an empty value is `""`. `get`, `scan`, `commit`, and `rollback` require `begin` or `begin_read`; writes require `begin`. Repeated begin requests are rejected without discarding the current transaction. Commit consumes its transaction even on conflict or persistence failure.

Ranges are `start <= key < end`; omit `end` or use null for no upper bound. Empty `start` begins at the first key. `after` is exclusive. Continue using `next_after` on the same transaction with the same range. An empty page can still have a continuation key. Writes staged in that transaction affect subsequent reads.

Read-only transactions can finish at their original snapshot. A writing transaction validates write, read, and range dependencies. Successful commit replies are sent only after durable log synchronization. Checkpoints preserve pinned old readers.

## Single-request operations

`get_latest`, `scan_latest`, `put_commit` and `delete_commit` accept the same fields
and bounds as `get`, `scan`, `put` and `delete`, respectively. They require no
active transaction and perform one internal transaction per request. An active
transaction causes `invalid_request` without discarding its staged changes.
Read responses have the original get/scan shape. Mutation responses contain the
committed `version` and are returned only after durable synchronization.

Each `scan_latest` request has its own snapshot. Its continuation key is a keyset
position, not a snapshot token: later pages may reflect intervening commits.
Use `begin_read` (or `begin`) + repeated `scan` + `rollback` for a stable multi-page snapshot.
Missing values remain null; empty values remain empty strings. Read failures
and invalid mutations release the internal transaction. A lost mutation response
still means the outcome may be unknown; requests are not automatically retried.

`ping` reads only the committed version, without traversing records or versions.
It still observes the engine's poisoned/reopen-required state.

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

Engine byte budgets may be explicitly configured using the options above. Values are positive decimal byte counts fitting the platform `usize`; zero, negative, suffixes and overflow are rejected. Omitted options retain their defaults. These options do not preallocate their stated capacity or automatically resize to container memory. Recovery also needs enough buffer and retained budget; an insufficient budget fails open without replacing the database.

Rust callers can use `Server::bind_with_limits(config, limits)`; `Config` and `Server::bind(config)` remain source-compatible with the same defaults. Explicit `max_transactions` is respected separately from the connection limit. The CLI continues to use its connection count as the transaction limit.

For example, `--max-retained-bytes 671088640` sets a 640-MiB **logical retained-data budget**, not a 640-MiB process limit. Choose it alongside an independently enforced container/OS memory cap and headroom for other allocations. It does not guarantee that a given database fits the container.

Engine defaults bound staged changes to 32 MiB, dependencies to 16 MiB, retained data to 128 MiB, and processing buffers to 16 MiB. These are not a strict RSS budget: network buffers, JSON allocations, OS resources, and recovery allocations also consume memory. The engine currently loads database contents into memory.

## Errors and recovery

Errors normally return `{"ok":false,"error":"code"}`. Codes include `invalid_request`, `limit`, `resource_exhausted`, `conflict`, `storage_error`, `commit_uncertain`, `checkpoint_uncertain`, `reopen_required`, and `unauthorized`.

Authentication failure, malformed frames/JSON, and expired deadlines close the connection. Overloaded connections are immediately closed rather than queued. Disconnects discard staged, uncommitted transactions.

**A lost commit response is not evidence of rollback.** The commit may have completed. Reconnect and inspect application-level operation identifiers before retrying. Automatic deduplication is not implemented. Persistence uncertainty can require restarting the service to reopen and recover the store.

### Default WAL and upgrades

The standard Rust library, C ABI, server and Docker image create new databases in managed WAL format 4. A normal `cargo build --locked --release --features service --bin catdb-server` or `docker compose up --build` includes the format support. No storage-mode setting is needed. A new managed database starts with a 1 MiB data area plus four 64 KiB recovery slots.

As pending updates approach 7/8 of the data area, the commit path starts a checkpoint. The replacement retains the current live records and concurrent commits. If the compacted data and waiting writes would use more than 3/4 of the capacity, the engine grows the replacement geometrically, up to 1 GiB minus the four recovery slots. At the maximum size, it attempts reclamation at the hard boundary. Once that limit or the available disk space is exhausted, writes can fail. Checkpoint publication requires temporary space for both files. The operation triggering maintenance can take longer, especially when a large live dataset must be copied. Explicit checkpoints are also available.

**Existing databases retain their storage policy**, including when reopened with `--init`. To migrate an existing database to automatic management, start the standard server once with `CATDB_MIGRATE_WAL=managed` or use Rust's `Database::checkpoint_managed_log()`. A successful checkpoint publishes the converted database atomically. Remove the setting after migration; reopening an already-managed database with it is idempotent. Previous releases may be unable to open a managed database. Retain a pre-migration backup if rollback to older software is required.

Explicit v3 and manually sized v4 files keep their configured capacity and checkpoint policy. The existing `CATDB_EXPERIMENTAL_FIXED_LOG_BYTES` and `CATDB_EXPERIMENTAL_REDO_LOG_BYTES` options select those explicit layouts. Set at most one of these options and the migration option. For a new file that must be interchangeable with legacy append-only builds, Rust exposes `Database::create_append` and the C ABI exposes `ndb_tx_db_create_append`.

For a group of up to 65,456 encoded WAL bytes, the writer stores the original group and two checksummed copies at separate offsets in the same file, synchronizes the file, and then publishes the commits. Larger groups synchronize the payload before synchronizing the end records. Recovery checks the checkpoint and frame sequence before applying repairs, synchronizes repairs before accepting writes, and scans the unused data area for abandoned bytes. An I/O failure during persistence makes the commit outcome uncertain and requires reopening the database. The recovery copies share one device; retain independent backups for media loss.


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

## Read-only snapshots and reclamation

`begin_read` pins a snapshot just like `begin`, but rejects `put` and `delete`
without ending the transaction. Reads do not register dependencies that would
only be needed to validate a later write. `get_latest` and `scan_latest` use this
path internally. Existing `begin` transactions keep their conflict checks.
This is a transaction mode, not an authorization role.

Rust exposes `Database::begin_read()` and `Database::collect_garbage()`.
Reclamation advances opportunistically when transactions begin or are dropped.
Each step visits at most 64 history keys and detaches at most 128 old versions,
with a target of 1 MiB of charged data (one larger value is allowed for progress).
Detached value allocations are freed outside the shared state lock and remain
charged until freed. A long-lived reader still pins its old versions.

An idle embedded application can call `collect_garbage()` until it returns false.
A true result means work remains, not necessarily that progress occurred: yield
or back off if persistence is active. These are work bounds, not a wall-clock
latency guarantee. Resource charges model retained data and buffers, not exact
allocator capacity or process RSS. Delayed reclamation can temporarily require
more retained-data budget. The C ABI is unchanged; the new explicit read-only
entry point is currently available in Rust and the service protocol.

## Opt-in background reclamation

Set `CATDB_BACKGROUND_RECLAMATION=1` when starting the service to move cooperative
version reclamation off transaction begin/drop. `0` (or unset) keeps the existing
cooperative policy; other values are rejected. This setting does not alter the
wire protocol, log format, commit synchronization or snapshot isolation.

Rust applications can call `db.set_background_reclamation(true)?`, and disable
it with `false`. Repeated enable does not start another thread. A failed thread
start returns an error and leaves cooperative mode available. One worker per
database owns shared state, not the database owner; shutdown joins it before
releasing the ownership file. Closing the last database handle can still take
time to stop the worker and free database memory.

The worker attempts the data lock without queuing behind active requests. It
runs at most 16 bounded portions with yields, then requests a 100 microsecond
pause. Lock contention or active persistence requests a 1 ms pause; no pending
work requests a 10 ms pause. These are requested sleeps, not timing guarantees.
Existing per-portion key/version/byte bounds and resource charges remain intact.
Worker shutdown wakes sleeping maintenance. The idle polling and thread stack
are additional resource costs; this feature is **disabled by default**.

`reclamation_pending()` checks reclaimable work and an in-progress detached
portion; it does not mean versions pinned by a live snapshot can be freed.
`reclamation_status()` reports mode, busy status, lifetime batch count (including empty checks), reclaimed
charged bytes, and maximum observed worker lock hold. Explicit `collect_garbage()`
remains synchronous and can assist even in background mode. Poll status with a
pause if waiting for background completion; do not busy-spin. Concurrent updates
can create more work after a false result.

Foreground calls can encounter resource limits while reclamation is deferred.
No unbounded reserve, hidden mutation retry or weakened durability is introduced.
The optional mode prioritizes request latency at the expense of total collection
time. See the [measured tradeoffs](benchmarks/reclamation-2026-11-05/README.md).
The C ABI remains compatible; this new setting is exposed in Rust and the service.

## Optional reclamation diagnostics

Rust `Database::reclamation_profile()` returns a separate `ReclamationProfile`,
preserving the existing `ReclamationStatus` shape. Build with `lock-timing` to
record lock misses, persistence deferrals, collection/free/yield wall time,
requested and actual pause time, and total worker lifetime. Otherwise these
new fields are zero. Worker lifetime is finalized when the worker exits; stop
or disable it before comparing summed intervals. Profiles are cumulative across
enable/disable cycles and do not reset on read.

These are elapsed durations, not CPU usage or a direct measurement of scheduler
latency. Diagnostic clocks and atomics themselves affect performance. The
[completion-time investigation](benchmarks/reclamation-cost-2026-11-05/README.md)
retains both timed and lighter comparisons. Its faster scheduling prototypes
were not adopted; the existing background policy and disabled-by-default setting
remain unchanged.
