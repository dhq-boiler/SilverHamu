# Single-sync recovery candidate: preliminary gate rejected

**No speedup is claimed. The data-only valid-prefix candidate failed the recovery
gate, so no performance comparison or production change was made.**

The preceding [I/O study](../io-phases-2026-11-07/README.md) observed shorter sync
times when SQLite reused existing WAL space. The earlier
[fixed-length prototype](../fixed-log-2026-11-05/README.md) used two syncs: payload,
then a separately persisted end record. This investigation asks whether scanning
self-validating records could replace that end record and permit one normal sync.

## Candidate and explicit fault model

The standalone Rust byte-image model uses 128-byte records containing a generation,
sequence, previous-record CRC, payload length, 64-byte synthetic payload, commit
footer, repeated generation/sequence, CRC32 and its complement. A conceptual
successful sync precedes publication. This is **not a file-backed DB prototype**:
there is no OS sync, real power failure, filesystem timing, transaction engine or
physical atomic-write assumption in this model. Its purpose is to find a recovery
counterexample before implementing those layers.

Two permitted histories are compared:

| History | Stable sequence 1 | Sequence 2 | Required outcome |
|---|---|---|---|
| Interrupted append | Valid | Only a prefix reaches storage; no success was returned | Recover sequence 1 automatically |
| Post-acknowledgement damage | Valid | Full record was synchronized and acknowledged, then its suffix becomes zero | Restore sequence 2 from a valid source or explicitly reject damage; never silently report sequence 1 as current |

For every cut from byte 1 through byte 127, those histories produce exactly the
same modeled disk bytes. A check at byte 64 prints both record images in the
[test output](test-output.txt). A checksum can detect the invalid second record,
but cannot identify which history produced it. Increasing the checksum width or
adding a sequence number alone cannot distinguish identical input images.

This fault model includes **post-sync corruption**, not just an ordinary process
crash. It does not imply that a successful fsync can lose data on every healthy
device, and the 32-byte fragments in the tests are synthetic units, not claimed
hardware sectors. A narrower crash-only contract or an atomic-write guarantee
changes the problem and would need explicit review rather than silently weakening
the approved safety condition.

## Observed result

- `DiscardInvalidTail` recovers sequence 1 for both histories. That silently loses
  an acknowledged update in the second history and fails the approved condition.
- `RejectAmbiguousTail` reports an error for both. It avoids silent loss, but fails
  automatic recovery of the interrupted append in the first history.
- Six Rust tests pass **as characterization/counterexample tests**. This means the
  limitations reproduce, not that the candidate passed the safety gate. The output
  explicitly reports `speed gate FAILED (expected counterexample)`.

The model also covers a valid chain, retained old values after publication, empty
and exact-capacity images, rejection without overwriting existing values, sync
failure without publication, wrong generation/sequence/chain, and combinations of
new fragments with zero or previous-generation suffixes. A fully persisted update
may be recovered even when its caller received a sync failure: lack of a success
response is not proof that no bytes reached storage.

## Decision and unexecuted work

The approved stop condition applies: do not benchmark a candidate that has already
failed its preliminary recovery contract. Consequently this iteration did **not**
run the 100k-row / 8,000-update / three-repeat performance comparison, real file
sync measurements, child-process termination, checkpoint/segment recycling,
production MVCC/conflict tests or existing-format/API migration tests. There are
no fabricated throughput, p99 or sync-count results. Model sync counters are not
OS measurements. No new performance chart replaces the previous measured chart.

The existing engine, defaults and file formats are unchanged. The current separate
end record can supply evidence about where a valid payload was expected, provided
that evidence remains trustworthy. This experiment does not certify the existing
two-sync implementation against arbitrary simultaneous data/metadata corruption,
nor prove that all single-sync designs are impossible.

## Next design work

The next candidate needs evidence or a repair source beyond the sole ambiguous
record: for example, independently verifiable redundant records under a stated
bounded-fault model, or an independently trustworthy durable boundary. Merely
putting a marker next to the payload before one sync does not establish ordering
between their persistence during an interrupted sync.

Before another throughput trial, specify the corruption/atomicity assumptions,
which source distinguishes or repairs the two histories, and the ordering needed
to reuse generations without treating old contents as new. Then test that protocol
against the same counterexample, including corruption of the evidence itself.
Additional writes, initialization, checkpoints and reclamation syncs must be
counted. The old end-record sync must not simply be removed from production code.

## Reproduce

Standalone source: [recovery_model.rs](../../../benchmarks/single-sync-gate/recovery_model.rs).
From the repository root (POSIX shell; adjust the bind path on Windows):

```sh
docker run --rm --network none --cpus 2 --memory 1g \
  -v "${PWD}/benchmarks/single-sync-gate:/probe:ro" rust:1.89-bookworm \
  sh -c 'rustc --edition=2024 --test /probe/recovery_model.rs -o /tmp/recovery-model && /tmp/recovery-model --nocapture --test-threads=1'
```

The container compiles into its disposable `/tmp`; the source mount is read-only.
No real data or production database is opened. [Environment/source digest](environment.json)
and test output accompany the report. This is a bounded preliminary investigation;
no unapproved long-running measurement or default change follows its failure.
