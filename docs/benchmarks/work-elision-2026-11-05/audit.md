# catdb work-elision audit — 2026-11-05

Baseline: `8b39b88`; both preceding production/documentation CI runs succeeded.
Scope: transactions, service dispatch, group commit, reclamation, transaction C ABI and export. No format change or PALV deployment is part of this batch.

| Priority | Work observed | Sufficient condition / proposed change | Obligations that remain | Status |
|---|---|---|---|---|
| 1 | Single-request dispatch deep-clones the JSON value just to replace `op` | Pass the internal operation name separately to the same dispatch body; borrow the original request | Same validation, active-transaction rejection, output bounds and durable acknowledgement | Implemented; measured in this batch |
| 1 | Every read/write dependency looks up the committed row even when generation is unchanged | Skip `State::changed` only when committed sequence equals transaction snapshot | Always inspect same-group earlier keys; retain validation order and advanced-generation checks | Implemented; measured in this batch |
| 1 | `export_values` begins a writable transaction and registers an unused range dependency | Begin a read-only transaction; export never stages/commits writes | Snapshot pin, output format, memory buffers, destination protection, errors and version | Implemented; measured in this batch |
| 2 | Full scans insert sorted stored/staged data into a temporary BTreeMap | Merge the two ordered streams directly into an ordered result | Shadowing, staged deletions, stable old versions, conservative resource accounting and off-lock copies | Deferred; larger change than the first batch |
| 2 | Retained growth searches keys during admission and again during publication | Carry a validated growth charge if intervening changes cannot invalidate it | Same-group distinct updates, recovery admission, failure poisoning and accurate budget | Deferred; requires a concurrency proof |
| 2 | Stage performs repeated gets/contains/insert and copies an existing key | Reuse an occupied entry after successful reservations | Resource rejection must preserve old staged values and accounting | Deferred; measure frequency before complexity |
| 2 | Explicit stats counts all current rows/versions and active pins | Incremental counters maintained on every transition | Exact counts after recovery, tombstone reclamation, checkpoint and transaction drop | Deferred; unlike ping, C ABI db_info actually returns these counts |
| 3 | Each group frame is issued in a separate write call | Consider vectored/coalesced writes if syscall overhead is measurable | Short writes, fixed-log boundaries, sync ordering and allocation budget | Deferred; do not simply remove writes/syncs |

## Operations that cannot simply disappear

- Same-group earlier updates are not reflected in the committed sequence until publication.
- `get` of a staged value still calls `state()`; bypassing it would bypass poisoned-state behavior unless an equivalent check exists.
- Read-only commit checks database state, and transaction disposal releases snapshot pins/budget. Returning immediately would change error/lifetime semantics.
- Identical-value writes or deletion of missing keys may still establish versions/conflicts. Suppressing them is not automatically equivalent to removing a redundant copy.
- Reclamation charges cannot be released before memory destruction. Prior structural/scheduling experiments already show regressions; they are not repeated in this batch.
- Copying a returned value is required by the existing owned-value API. Removing that copy needs a lifetime/API design, whereas removing the intermediate JSON clone does not.

## Design review

- KISS/DRY: reuse the same operation body without cloning and rewriting the entire request. Do not duplicate protocol validation for single-request operations.
- Command/query separation: export is read-only; use the existing read-only mode rather than recording write-validation dependencies that cannot be consumed.
- Keep the generation predicate local to validation. No new cache, persistent metadata, caller-visible protocol, or general-purpose framework is needed.

The user approved the first-batch QA scope before test implementation. All three changes are retained with the limits and unfavorable results described in README.md. Deferred candidates are not implemented; future cost and completion dates are unknown. Work elimination is not evidence that every operation becomes faster.
