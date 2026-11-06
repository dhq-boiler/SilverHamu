# Variable-length production-codec gate (2026-11-07)

## Decision

The bounded codec-integration gate passed on Linux. This is a **test-only** dual-copy
store using the production `encode_frame` and `decode_frame`, not a new production
persistence mode. Defaults, public APIs and existing database formats are unchanged.
No throughput or latency comparison was run. The next decision should consider
space/reuse costs before connecting this protocol to the production transaction engine.

## Contract and limits

At most one detectably damaged copy of each acknowledged record or active selector
pair is allowed before repair completes. Independent copy locations are not independent
failure domains. CRC32 detects the tested corruption; it is not authentication, and
checksum collisions, malicious rewriting, both-copy damage and device-wide loss are
not covered. The both-copy-loss counterexample remains a passing negative-control test:
recovery can silently discard that acknowledged transaction outside this contract.

A successful ordinary append follows both writes and one successful `sync_data`.
Recovery may include a complete transaction whose caller did not receive success;
it must not return a partial transaction. A failed persistence phase poisons the
writer until reopening and repair. Phase-hook failures are not real kernel sync failures.
Child-process termination is not physical power loss or device-cache testing.

Each independent record slot is 16,384 bytes, with a 64-byte envelope and up to
16,320 bytes of production WAL frame. The fixture has two banks, each with one
checkpoint and four transaction slots, plus four selector slots: 393,216 reserved
file bytes. The **whole checkpoint must fit one slot**. This is deliberately a
small bounded gate, not a scalable checkpoint design. Oversized transactions and
checkpoints are refused before persistence; production 64-MiB frames are not stored.
Empty change sets do not consume a slot, sequence or sync.

Old reads are retained `Arc<BTreeMap<...>>` snapshots in the test store, not integrated
production MVCC readers, conflict validation, resource admission or group commit.
Existing production tests were run separately; their passing does not establish
that this new protocol is integrated with those features.

## Protocol

1. Validate lengths/counts and slot capacity; encode with the production codec.
2. Prepare the complete next visible map before writing either copy.
3. Write the two variable-length records at independent fixed offsets, sync, publish.
4. Reopen reads bounded slots and validates envelope, inner frame and operation order.
   It constructs the entire recovered map before returning a writable store.
5. Repair invalid peer copies and sync data repairs before selector repairs. No update
   API is available until repair finishes.
6. Switch by clearing/preparing the inactive bank and syncing its variable checkpoint,
   then publishing/syncing a new selector pair while retaining the old pair, then
   clearing/syncing the old bank. Selector pairs alternate across generations.

Only meaningful bytes are compared for repair. Stale bytes after a shorter record
are ignored; fixed offsets preserve the peer location when a length field is damaged.
Reading allocates a bounded full slot before inspecting its header. This does **not**
mean recovery allocates nothing before validating a length.

## Coverage and results

- 11 new tests passed; one ignored helper is invoked by the external-kill test.
- 8,450 selector prefix combinations across initial switch and selector reuse.
- 100 append prefix combinations at header/page/end boundaries, plus 10 peer-repair
  prefixes. These are selected boundaries, not exhaustive arbitrary byte subsets.
- 4,269 single-bit mutations, one at every meaningful byte of a fixture record,
  each restored from its unchanged peer.
- 30 external kills: append 7, repair 9, switch/reclamation 14. Each reopens and
  performs another durable update. Synced append barriers require the new whole map.
- Empty and binary keys/values; mixed insert/update/delete; retained old snapshots;
  repeated switches; 4-KiB page crossings; exact slot fit and one byte over;
  1023/1024/1025-byte keys; production value/count/64-MiB body limits; sequence/epoch
  exhaustion; oversized checkpoint rejection and subsequent recovery by deletion.
- Valid outer envelopes cannot hide invalid inner count/key/value lengths or prefix.
  Disagreeing valid copies fail closed. Clean reopen does not repeatedly repair padding.

The large production count/value/body boundary cases exercise preflight validation,
not large on-disk records. Key lengths through 1024 also round-trip through the codec.
Tests are compiled only on Linux because this experimental harness uses directory
sync semantics. Windows runtime behavior was not tested by this new probe.

See [raw build/test output](test-output.log). The final build also runs default and
`service,wal-direct` library tests, including existing failure, concurrency and recovery
checks: **59 passed / 8 ignored** for defaults and **74 passed / 10 ignored**
for `service,wal-direct`. Ignored entries are explicit benchmarks or subprocess helpers;
no test failed. The targeted run is 11 passed / 1 ignored / 55 filtered out.

## Write and sync accounting

These are code-path byte counts and successful-run sync-call counts, not physical
storage traffic or timing. The fixture makes five appends and three switches.

| Category | Bytes | Sync calls |
|---|---:|---:|
| Initialization (including directory sync) | 393,216 | 2 |
| Five appends, both copies | 43,490 | 5 |
| Three inactive-bank clears plus checkpoint copies | 517,746 | 3 |
| Three selector-pair publications | 384 | 3 |
| Three old-bank reclaims | 491,520 | 3 |
| Fresh repair of one changed transaction copy | 4,369 | 1 |
| Selector repair alone after an interrupted data repair | 64 | 1 |

The five append totals split into **21,070 application key/value bytes**, 355 bytes
of production framing/operation metadata, another 21,425-byte encoded-frame copy,
and 640 bytes of dual envelopes. One transition of this fixture writes **336,550
bytes and needs three syncs**. Clearing/reclaiming the entire bounded banks dominates
these bytes. At four appends per transition the amortized count is 1.75 syncs/update,
excluding initialization and repairs. One ordinary-append sync is not one amortized sync.

Named preparation-buffer measurements: maximum returned encoded Vec length/capacity
is **4,307 / 4,307 bytes**; checkpoint cloned key/value payload is **4,236 bytes**.
Each encoded fixed block is 16,384 bytes and each record carries a 16,320-byte payload
array. Initialization uses a 393,216-byte contiguous image buffer in addition to the
slot image. Recovery holds a full slot image and may retain repair blocks.
These are **not total peak allocated memory or RSS**: encoder body temporaries, map
nodes, reconstructed/cloned maps, repeated stack record copies, allocator overhead
and old snapshots are not included in the Vec metric. No global memory-budget claim.

## Reproduction

From the repository root, with Rust 1.89 on Linux:

```sh
cargo test --locked --release --lib variable_gate -- --nocapture --test-threads=1
cargo test --locked --release --lib -- --test-threads=1
cargo test --locked --release --lib --features service,wal-direct -- --test-threads=1
```

Or build `benchmarks/variable-gate/Dockerfile` with the repository root as context.
The recorded run used Docker Desktop Linux and rust:1.89-bookworm
(image SHA256 096b9dd60acf6140d936a45412498b7b18338cdf1604f0efa8dcc215a36161cc).
LF-normalized source checksums (Windows checkout line endings excluded) are in [source-sha256.txt](source-sha256.txt).

## Remaining gates

The approved bounded investigation is complete, not production adoption. Before
large performance comparison: design scalable variable checkpoint storage and bounded
preparation/repair budgets; account for zeroing/reuse traffic; then integrate production
MVCC/conflict/group-commit rules and crash-test that integration. The bounded fault
assumption must stay explicit. Both-copy damage is still not solved.

Kudaki task reflection is pending: list_documents returned a session-initialization
error. No WBS file was edited directly. Continue within the existing persistence
experiment L1/L2 rather than creating a task for each test run.
