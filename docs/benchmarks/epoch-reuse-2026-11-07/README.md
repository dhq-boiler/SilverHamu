# Epoch-based reuse without whole-bank clearing (2026-11-07)

## Decision

The **corrected bounded test-only candidate** passed the selected recovery models
and file tests while reducing transition writes. Simply deleting the clear loops
was **not correct**: the first run failed during torn-slot reuse. Both failed/intermediate
results are retained below. No production engine, API, format or default was changed.
No latency, throughput or large-dataset experiment was run.

## Matched accounting

Same small variable-length dataset, five ordinary appends, three transitions, same
production WAL codec, same fixed 16-KiB slots and four append slots per bank.
The existing clear mode and the reuse candidate share code and the corrected recovery
validation. Counters represent bytes submitted by this harness and sync calls, not
physical device traffic. The byte counts for clear mode match the previous experiment.

| Per transition | Clear both banks | Retain obsolete bytes |
|---|---:|---:|
| Inactive-bank clearing | 163,840 B | 0 B |
| Two variable checkpoint copies | 8,742 B | 8,742 B |
| Selector pair | 128 B | 128 B |
| Old-bank clearing | 163,840 B | 0 B |
| Total | **336,550 B** | **8,870 B** |
| Sync calls | **3** | **2** |

Transition writes fall **97.36%** in this fixture. Three transitions total
1,009,650 versus 26,610 bytes, with 9 versus 6 sync calls. Five ordinary appends
remain 43,490 bytes / 5 syncs; initialization remains 393,216 bytes / 2 syncs.
Including initialization and appends, submitted writes are 1,446,356 versus 463,316
bytes; do not apply the transition-only percentage to total traffic.

File length stays **393,216 bytes** in both modes; this does not shrink the file or
promise secure deletion. Obsolete payload bytes remain until overwritten. Each
checkpoint still must fit one slot. At four appends per transition the amortized
sync count would be 1.75 versus 1.5 per update, excluding initialization and repair.
There is no measured speedup claim. Named frame Vec length/capacity peaks remain
4,307 bytes and checkpoint cloned payload remains 4,236 bytes in this fixture.
Full inner validation adds decoding work; recovery CPU/allocation overhead was not
measured. These named buffers are not total memory or RSS.

## The failure and its correction

The outer envelope originally checks CRC32 of the whole inner frame. That inner
frame already contains CRC-protected prefix/body codewords. A controlled pair of
otherwise identical frames with different transaction sequences has the same outer
payload CRC. Consequently a new complete envelope plus a stale complete inner frame
can pass `Record::decode` but fail the inner sequence check. This is a reproducible
construction in this encoding, not an assumed rare random bit-flip collision.
It does not establish a flaw in the production WAL's standalone recovery protocol.

1. The naive no-clear candidate failed with `inner sequence or checkpoint deletion`.
2. Validating each inner copy before choosing a peer allows a fully valid peer to
   recover. However, refusing every pair with no current valid inner frame still
   rejected **20 legitimate interrupted overwrite states** in the model.
3. The final experiment classifies a record as stale only when the outer record is
   valid and the entire inner frame can be decoded/validated with an **older**
   transaction sequence. This applies only to transaction slots, not checkpoints.
   If every outer-valid candidate is such a validated stale payload, the slot is
   treated as an unfinished tail. A later valid record after that gap is an error.
   Arbitrary invalid inner data, a future sequence, missing selected checkpoint,
   or disagreeing valid current peers still fail closed. A fully valid current peer
   is preferred and its other copy repaired before returning writable.

This classification relies on the existing **at most one detectably damaged copy
per acknowledged record before repair** assumption and monotonic non-wrapping
sequences. Under it, an acknowledged transaction has a valid current peer, so the
stale-only case cannot discard an acknowledged transaction. Both-copy damage remains
an explicit loss counterexample outside the contract. Checksums are not authentication.
This is a bounded experiment, not a proof over arbitrary failures or all persistence orders.

## Coverage

The added `reuse_*` tests cover:

- EQ: matched writes/results and retained old snapshots in both modes.
- BV: append-slot capacity, oversized checkpoint rejection before writes, epoch
  exhaustion, repeated long/short records and selector/bank reuse.
- ST: eight repeated full-log reuse rounds; interruption followed by reopen, another
  update and retry; no repeated repair on clean reopen.
- ER/model: **12,675 selector-prefix combinations**, **60 checkpoint-prefix cases**,
  **192 append-prefix pairs**, six damaged-current-selector cases. Corrected model:
  **zero refused states** among these cases. The nested-CRC example is retained.
- ER/file: **44 external child-process kills** across two reused-bank layouts:
  switch 20, append 14, repair 10. Reopen checks whole-map results and then performs
  another update and transition. The original 30 kill cases also remain.
- Guard checks: malformed inner fields, future inner sequence, a valid record after
  a stale gap and inconsistent selected checkpoint remain rejected. Repair failure
  prevents writable open. Both-copy-loss negative controls remain.

Old reads are test-store `Arc` map snapshots, not integrated production MVCC.
Process kills do not emulate power loss or a volatile device cache. Phase-hook errors
are not actual kernel sync failures. Prefix models are selected persisted states,
not exhaustive arbitrary-byte write reordering. One-copy faults are not independent
failure domains merely because offsets differ. No Windows run was made.

## Results and reproduction

Final targeted suite: **19 passed / 2 ignored helpers** (8 added tests).
Default library suite: **67 passed / 9 ignored**. Service + direct-codec library:
**82 passed / 11 ignored**. All final runs have zero failed tests. Helpers are
invoked by parent tests; other ignored entries are explicit performance experiments.
The source is test-only and shares the existing `variable_gate` module.


[Final build and test output](final-output.log), [initial failing run](initial-failure.log),
[intermediate peer-validation-only result](peer-only-output.log).
The intermediate suite explicitly asserted the counterexample; its passing tests
were **not** a safety-gate pass. Historical failed variants are provided as `.rs.txt`
files for audit, not compiled into the current test module.

From the repository root on Linux / Rust 1.89:

```sh
cargo test --locked --release --lib variable_gate -- --nocapture --test-threads=1
cargo test --locked --release --lib -- --test-threads=1
cargo test --locked --release --lib --features service,wal-direct -- --test-threads=1
```

Alternatively use the existing `benchmarks/variable-gate/Dockerfile` with the root
as build context. No large performance tests are invoked. LF-normalized source hashes
are in [source-sha256.txt](source-sha256.txt); logs have trailing whitespace trimmed.

## Remaining work

The bounded gate supports investigating reuse further, not adoption. A scalable
checkpoint design, explicit global buffer/repair limits, and integration with real
MVCC/conflict/group-commit behavior remain separate gates. The CRC composition issue
also motivates reviewing a digest that binds envelope identity and payload without
this codeword interaction. That design was not implemented here. Performance must
still be measured after a suitable safety gate; fewer submitted bytes do not guarantee
lower commit latency.

Kudaki reflection is pending due to the same MCP session-initialization error.
No WBS file was directly edited. Add results to the existing persistence experiment
L1/L2; do not create one task per run.
