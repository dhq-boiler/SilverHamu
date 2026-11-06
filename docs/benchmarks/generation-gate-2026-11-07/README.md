# Dual-copy repair and generation transition probe

**The bounded standalone repair/transition gate passed. No production integration,
unrestricted durability certification or performance improvement is claimed.**
The prior assumption remains explicit: at most one damaged copy of each
acknowledged record/control record before repair. Correlated loss of both copies
is outside that contract and remains a negative control.

This extends the [dual-copy probe](../dual-copy-gate-2026-11-07/README.md).
The experiment stores four synthetic byte-valued keys, four append slots per
generation, two data banks, and four generation-selector slots. It is not the
production WAL format or transaction engine.

## Design correction: preserve the old selector pair

Overwriting the only two generation selectors before one sync can leave **both**
partially persisted during a crash. The test suite retains a counterexample for
that design. The candidate instead alternates between two selector pairs:
the current pair remains untouched while the next pair is written. This is four
selector slots total, not two copies overwritten in place.

Normal append writes the new record twice, synchronizes once, and only then
publishes its values. A generation transition uses the following order:

1. Clear the inactive data bank, write the new checkpoint twice, then sync data.
2. Write both selectors in the inactive selector pair, then sync selectors.
3. Select the new generation in memory, clear the old data bank, then sync reclaim.

The old selector pair may remain valid while its data bank has been reclaimed.
Recovery selects the highest valid generation, checks matching identities and
sequence chains, and rejects a missing selected checkpoint rather than falling
back silently to an older one. The retained older selector pair is overwritten
only on a later transition while a newer stable pair still exists.

## Repair and update admission

Reopen validates the complete selected checkpoint/log before changing anything.
Only noncanonical copies are repaired; the healthy copy is not overwritten.
Data repairs are synchronized first, then any selected-selector repairs are
synchronized. Writable state is returned only after those steps complete.
Errors during append/switch poison that writer; failed reopen returns no writable
store. Failure before capacity or sequence/epoch checks changes no file bytes.

The old read snapshot is an immutable in-memory `Arc<[u8; 4]>`; it retains its
values across append and recycling. This does **not** test readers retaining
references to reclaimed disk pages or production MVCC/conflict handling.

## Verification results

Final Rust compilation with warnings denied and the suite passed **13 tests**.
One ignored test is the child-process entry point; the parent explicitly invokes
and kills it 37 times. It is not a skipped recovery case.

| Perspective | Result |
|---|---|
| Repair persistence model | All 65 prefix lengths of a repaired copy preserve the healthy copy and acknowledged values; conflicting valid copies reject |
| First transition | 4,225 combinations of partial new-selector copies recover the acknowledged state using the retained old pair or complete new selector |
| Selector slot reuse | Another 4,225 combinations overwrite stale generation-1 selectors while generation 2 remains readable |
| Current selector damage | 130 lane/suffix cases select the current generation, not the reclaimed older bank |
| Reclamation / stale contents | Partial old-bank clearing and stale-generation records do not change selected values |
| Selected checkpoint missing | Recovery rejects the inconsistent state without silently using an old checkpoint |
| Negative controls | Both data-copy loss can silently lose the last update; overwriting the only selector pair can remove all restart selectors |
| Real-file repair interruption | Nine barriers cover partial/full repair writes, before/after sync of data/selectors and writable-open admission |
| Real-file switch interruption | Fourteen barriers run on each of two cycles, including stale old selectors and an old bank deliberately left unreclaimed in the second-cycle fixture |
| Injected repair failures | Six phase errors prevent writable-open success; a subsequent clean reopen restores the acknowledged state |
| Injected switch failures | Six phase errors poison the writer; reopen retains acknowledged values |
| Capacity / overflow / old reads | Exact capacity, create-new refusal, `u64::MAX` epoch/sequence rejection, three successive transitions and retained old values pass |
| Continued operation | After every external kill, reopen restores all four values, a further update succeeds, and another reopen sees it |

Model fragments and prefix images are synthetic byte-level scenarios, not claims
of physical sector atomicity. The failures are injected immediately before/after
I/O phase boundaries, **not kernel-generated fsync errors or real disk-full tests**.
Process killing leaves the OS cache alive and is **not a power-cut test**.
Successful sync is assumed to honor the storage contract. Testing finite fixtures
does not establish a universal proof for arbitrary corruption or schedules.

## Measured code-path costs

These are application write-byte and executed synchronization-call counters,
not timings, OS trace counts, or physical-device write amplification.

| Operation | Application bytes written | Synchronization calls |
|---|---:|---:|
| Initialization | 98,304 | 2: file + containing directory |
| One ordinary append | 128 | 1 |
| New-generation preparation | 768 | 1 |
| New-selector publication | 128 | 1 |
| Old-bank reclamation | 640 | 1 |
| Repair one data copy | 64 | 1 |
| Repair one selected selector | 64 | 1 |
| Clean reopen | 0 | 0 |

A full transition therefore writes **1,536 bytes and synchronizes three times**.
At four ordinary updates per transition, the steady-state code-path sync count
is `1 + 3/4 = 1.75` per update, excluding initialization and repairs. This is a
cost calculation, not a measured latency or a prediction for larger databases.

The repeated-transition test records 13 appends (1,664 bytes / 13 sync attempts),
three preparations (2,304 / 3), three selector publications (384 / 3), and three
reclaims (1,920 / 3), plus initialization. A fixture requiring both data and
selector repair records two additional syncs. The zeroing strategy is deliberately
conservative; future optimizations must preserve the ordering tested here.

Writes are split into two halves to expose interruption points. Fault-fixture
corruption writes and their explicit syncs are test setup and are not included
in the ordinary-operation table. All costs apply only to this small fixed-format
probe with synthetic complete-state records.

## Remaining work and scope limits

- No catdb production source, default, API or existing database file was changed.
- No 100k-row benchmark or throughput/p99 chart was generated in this stage.
- Variable-sized keys, values and transactions, concurrent updates/readers,
  real engine conflict detection, resource budgeting and platform compatibility
  have not been integrated.
- Both-copy corruption, malicious checksum-preserving changes, shared device
  loss and false storage success reports are not covered. Four selector slots
  in one file do not create independent physical failure domains.
- Initialization interruption, arbitrary kernel I/O failures and real power-loss
  recovery are not certified by these tests. No broader original fault guarantee
  is inferred from the approved bounded model.

The next useful gate is realistic record/transaction integration under the same
explicit assumptions. A later performance comparison must include initialization,
repair, generation transitions and reclamation rather than counting only the
single-sync normal append. Adoption remains a separate decision.

## Reproduce

[Source](../../../benchmarks/generation-gate/probe.rs),
[final test output](final-output.txt), [source digest and environment](environment.json).
From the repository root with a POSIX shell (adjust mount syntax on Windows):

```sh
docker run --rm --network none --cpus 2 --memory 1g \
  -v "${PWD}/benchmarks/generation-gate:/probe:ro" rust:1.89-bookworm \
  sh -c 'rustc --edition=2024 --test -D warnings /probe/probe.rs -o /tmp/gen-probe && /tmp/gen-probe --nocapture --test-threads=1'
```

Fixtures and compilation stay inside the disposable container's `/tmp`. Source
is mounted read-only, files are create-new-only, and existing user libraries are
not opened. Model tests passed before the first real-file phase tests; the final
suite includes the second-cycle and negative-control additions.
