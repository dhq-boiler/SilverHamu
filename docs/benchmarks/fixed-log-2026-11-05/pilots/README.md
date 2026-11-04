# Superseded layout experiments

`adjacent-end-records` is the first complete paired run (1/4 clients, three
repeats), before placing the two logical-end copies on separate 4 KiB pages.
Its format used capacity + 128 bytes. It passed process-exit tests but adjacent
copies should not be treated as separate recovery pages, so it was not retained.
Do not open these preliminary-format files with the final prototype.

`short-adjacent-diagnostic` is a 1,000-operation, one-repeat diagnostic preflight,
not a main result. The final comparison uses 8,000 operations per case.

The source snapshot in `adjacent-source.json.gz` overlays commit 06bd70b:
`fixed_log.rs` and `transactions.rs` go under `src/`, and `run.py` under
`benchmarks/fixed-log/`. Remaining runtime sources/features are the same.
Image IDs and source hashes are retained in each environment record.

Earlier harness setup checks found no `/proc/1/io` support (and an initially empty cgroup
`io.stat`) in this WSL kernel, and corrected `/data` ownership so the non-root
server could create checkpoint files. These were setup failures, not discarded
performance comparisons. A later write probe showed that cgroup counters do
populate; supplemental I/O runs collect them separately. The main byte metric
is application WAL writes, not physical storage write amplification.
