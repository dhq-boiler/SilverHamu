# Dual-copy single-sync probe — bounded-fault preliminary result

**The approved bounded model and small file-backed probe passed. This is not a
production safety pass, a replacement for the existing WAL, or a measured speedup.**
The user explicitly approved the narrower assumption: after acknowledgement,
damage affects at most one of the two copies of each record before repair.
Both-copy loss remains an explicit failing-contract counterexample.

This continues the [rejected single-copy prefix design](../single-sync-gate-2026-11-07/README.md)
and the [observed log reuse/sync cost difference](../io-phases-2026-11-07/README.md).

## Protocol exercised

1. A fresh, create-new-only file is fully zero-written, with a generation/capacity
   anchor, then synchronized; its directory is synchronized separately.
2. Each record contains the generation, sequence, prior CRC, a synthetic 64-byte
   payload, commit footer, repeated identifiers, CRC and complementary CRC.
   Two identical 128-byte records are placed in different 4 KiB slots in the
   **same file**, in lanes separated by 1 MiB. These are logical offsets, not
   proof of independent physical failure domains.
3. Both writes precede one real `File::sync_data()` call. Publication to the
   in-process visible snapshot follows successful sync. Any write/sync-path error
   leaves the writer poisoned; capacity rejection leaves existing content intact.
4. Recovery validates the anchor, both copies, sequence and chain. Matching valid
   copies or one valid current-generation copy can recover a record; conflicting
   valid copies and a valid record after a missing pair are rejected. Stale
   generations do not become current records.
5. Before returning a writable reopened store, any accepted record's damaged copy
   is rewritten from the valid copy and synchronized. A later single-copy fault
   must not consume redundancy already lost during the first fault.

The anchor is checked against this probe's fixed epoch and capacity. Damaged or
unsupported anchors are rejected without modification; anchor repair, generation
rotation, checkpoint publication and segment reuse are **not implemented**.
The format is a four-record standalone experiment using repeated-byte payloads,
not catdb's key/value API or production concurrency machinery.

## Results

The final warning-as-error Rust build and suite pass **10 tests**. One ignored
test is a child-process entry point, explicitly launched four times by its parent
test; it is not an unexecuted safety case.

| Perspective | Evidence |
|---|---|
| Interrupted writes | All 129 × 129 = 16,641 pairs of prefix-persistence lengths retain the previously acknowledged record; a complete valid new copy may also be recovered |
| One-copy damage | 256 lane/suffix-cut cases recover the acknowledged value from the other copy |
| Fragment order / stale contents | Synthetic 32-byte fragment combinations with old-generation contents preserve the intact copy; no hardware-sector atomicity is assumed |
| Conflicting evidence | Two valid but different copies, wrong sequence, broken chain and holes are rejected |
| Negative control | Damage to both acknowledged copies still lets the prefix scanner lose the last update; this remains outside the bounded contract |
| Real file contents | Empty/exact-capacity/reopen/old in-process snapshot checks pass; existing files are not overwritten |
| Repair | Corrupt either lane, reopen and repair with one extra sync, then corrupt the other lane: the original value still recovers |
| Anchor damage | Corruption of the fixed generation anchor fails open without modifying the file |
| Failure injection | An error injected immediately before sync does not publish success and poisons the writer; this is not an injected kernel fsync failure |
| External termination | Parent kills child after copy A, after copy B, after sync and after publication; prior acknowledged value survives all four, synchronized new value survives the last two |

All four process-kill runs recovered both values `[11, 22]`, including the two
pre-sync points. That is permitted: no success response does not imply no bytes
persisted. Process death leaves the kernel/page cache alive, so this is **not a
power-cut durability demonstration**. Prefix and reordered-fragment cases are
byte-image simulations, not physical disk snapshots.

The pre-sync hooks observe the old published length; the post-publication hook
observes the new length. Old values are held through `Arc` snapshots in this
standalone probe, not through production MVCC/conflict handling.

## Synchronization and write costs are not hidden

| Operation | Calls executed by probe code |
|---|---|
| Fresh initialization | One file `sync_all`, plus one directory `sync_all` |
| Normal append | Two 128-byte writes, then one `sync_data` before publication |
| Reopen without repair | No repair sync |
| Reopen repairing one or more copies | Repair writes, then one extra `sync_data` before returning writable state |
| Fault injection setup | Separate explicit sync after overwriting bytes to create the corruption fixture |

These are code-path counts with actual standard-library synchronization calls,
not an OS syscall trace or throughput measurement. The file is initialized to
1,069,056 bytes for four record pairs. Future checkpoint/recycling and repair
costs must be included in a performance comparison. No 100k-row benchmark was
run in this stage, and no earlier performance chart is replaced with model data.

## Assumptions, limitations and next gate

- A successful synchronization is assumed to honor the storage contract. Two
  copies in one file do not protect against drive loss, correlated corruption,
  lost acknowledgements at every layer or a device falsely reporting persistence.
- The bounded fault model applies to acknowledged record copies. Checksums are
  corruption detection, not authentication or proof against adversarially forged
  records. The all-copy counterexample is retained, not reclassified as success.
- The probe repairs a single damaged copy before resuming writes. Interruption or
  failure **during that repair** has not yet been modeled or killed at each step;
  do not infer that coverage from the normal-append kill tests.
- Complete generation transition, anchor replacement, reuse, checkpoint safety,
  retention across recycling, production transactions/conflicts, cross-platform
  behavior, resource budgets and real storage/power failure remain unverified.
- The current production default and existing file formats are unchanged. This
  result does not satisfy the original unrestricted corruption requirement.

The next bounded design gate should cover repair interruption and generation /
checkpoint / reuse ordering, including all additional synchronization and both-copy
failure controls. Only after those pass should a separate approval scope cover
realistic workload integration and a fair 100k-row performance comparison.

## Reproduce and evidence

[Standalone source](../../../benchmarks/dual-copy-gate/probe.rs),
[final output](final-output.txt), [environment and source hash](environment.json).
From the repository root using a POSIX shell (adjust the mount path on Windows):

```sh
docker run --rm --network none --cpus 2 --memory 1g \
  -v "${PWD}/benchmarks/dual-copy-gate:/probe:ro" rust:1.89-bookworm \
  sh -c 'rustc --edition=2024 --test -D warnings /probe/probe.rs -o /tmp/dual-probe && /tmp/dual-probe --nocapture --test-threads=1'
```

Compilation and all fixtures live in the disposable container's `/tmp`.
The source bind is read-only; real libraries and existing data are never opened.
Model-only (`model_`) tests passed before file-backed (`file_`) tests were run.
The final suite includes the subsequently added copy repair and damaged-anchor
checks. An optional formatting attempt found no `rustfmt` in the image; it did
not change source or run tests. Final compilation succeeded with warnings denied.
