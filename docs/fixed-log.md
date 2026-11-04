# Experimental fixed-length log

This opt-in prototype is separate from the standard append-only WAL. Build with
`service,fixed-log` and explicitly set `CATDB_EXPERIMENTAL_FIXED_LOG_BYTES` to an
aligned capacity, for example `67108864` (64 MiB). Use disposable data. Opening a
legacy database with this variable converts it by checkpoint replacement; it is
not a read-only flag. Builds without `fixed-log` reject version 3 files.

The Rust API `Database::checkpoint_fixed_log(capacity)` converts or expands a
log through the same synchronized, atomic replacement path as a checkpoint.
Existing transaction and service operations are unchanged. Existing version 1/2
logs remain supported and new databases remain append-only without the explicit
conversion. The service variable must match an already fixed-length database;
it does not silently resize it on restart.

## Layout and commit protocol

- Existing 48-byte WAL header, with version 3 and its checksum.
- Existing checksummed transaction frames; logical end is independent of EOF.
- At least 20 bytes reserved for a zero end sentinel inside the data capacity.
- Two 64-byte end records, each in its own 4 KiB metadata page, after the data
  capacity. Each binds generation, logical end, capacity, database ID and header
  checksum under its own CRC32. Physical size is capacity + 8 KiB.

Capacity must be a multiple of 4 KiB, at least 4 KiB and no more than the existing
1 GiB WAL limit minus 8 KiB. Initialization writes zeros, not just sparse extents,
then synchronizes the complete file before publication. Initial creation,
checkpoint preparation and directory publication retain their stronger existing
synchronization. Fixed-length commits do not grow the file.

Each commit group performs:

1. Write complete frames and the zero end sentinel.
2. `sync_data` the payload before writing a new logical-end record.
3. Write the next generation into the alternating metadata page.
4. `sync_data` again, then publish in-memory versions and send successful replies.

There are **two durable sync calls per group**. The second barrier is included in
all measurements; the payload-only time is not advertised as commit latency.
Moving the end record before the first sync would allow a crash to persist an
end that refers to incomplete payload, so this prototype does not do that.
Separate pages avoid relying on one page's two adjacent copies as independent
recovery records; neither page writes nor devices are assumed infallible.

Rust documents that [`sync_data`](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_data)
may omit metadata not needed for file contents and can map to `sync_all` on some
platforms. Performance measurements here are on Docker Desktop's Linux/WSL
filesystem; Windows correctness tests do not imply the same speedup on Windows.

## Recovery and failures

Recovery validates the header and end records, then checks every frame through
the newest valid end. Corruption inside that confirmed region is rejected;
it is not silently rolled back to an earlier end. Complete, valid transactions
beyond that end may be replayed (their callers may not have received success).
An incomplete tail beyond the confirmed end is discarded. No transaction is
partially replayed. A repaired sentinel and end are synchronized before use.

One damaged end copy is recoverable by validating the other copy and scanning
complete frames beyond it. Both damaged copies cause a visible open failure.
The CRC checks detect accidental corruption; they are not cryptographic integrity
or a guarantee against simultaneous arbitrary damage to payload and metadata.
I/O or synchronization failures poison the handle and return an uncertain commit
outcome, requiring reopen/inspection before retry. Old snapshots retain their
in-memory versions throughout successful commits and checkpoint replacement.

A full data region rejects a write before modifying the log. Expanding it requires
an explicit checkpoint into a newly initialized file; in-place relocation of end
records is intentionally avoided because a crash could lose their location.
Ordinary checkpoints keep the chosen capacity. Conversion/expansion may block
writers during final zero filling/publication, and needs temporary disk space
for both files. Reopen currently performs two repair syncs even on a clean log.

`commit_sync_attempts()` counts logical durability attempts. In this experimental
mode each successful attempt uses two physical sync calls. Diagnostic builds
with `lock-timing` additionally expose aggregate payload/end synchronization
times via the experimental `fixed_log_metrics` service operation.

## Verification boundary

Tests cover old snapshots, concurrent conflicts, capacity and expansion,
partial prefixes/bodies/seals, torn end records, individual metadata-page loss,
confirmed-payload corruption, initialization/sync failure, no early response or
visibility after payload sync, and subprocess exits at both barriers and around
checkpoint publication. Service tests also kill a four-writer process and verify
acknowledged values after restart. Actual power cuts and arbitrary device failure
have not been tested. This remains an Experimental format and is not a migration
recommendation for real libraries.
