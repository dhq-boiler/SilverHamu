# Optional inline index keys

Scope: only persistent State.rows keys, not transaction writes or history_keys.
Bytes of length 0..15 live in the key object; longer keys use Box<[u8]>.
Ordering and borrowed lookup use byte slices, never enum discriminants. No unsafe
representation code. The key object must not exceed Vec<u8> size in layout tests.
The public API, WAL/checkpoint format, logical budgets and default are unchanged.
Conservative retained accounting is deliberately not reduced before independent
memory evidence. Old generations continue through existing VersionChain storage.

Build both images from the same source with benchmarks/inline-key/Dockerfile,
using EXTRA_FEATURES=,inline-key only for the candidate. It builds the matching
embedded library, service and isolated allocation probe. Run run.py --out NEW/raw
for six alternating 1M/256B/1CPU/512MiB trials, 8000 read +8000 mixed requests,
old snapshot held, 1000 arrivals/s, and per-run safety/kill/reopen checks.
run.py stops on failure rather than silently repeating a failed condition.

Run diagnose.py --out NEW/allocations separately, after service measurements.
Its global allocator counts allocation calls and requested live bytes during open.
The diagnostic binary is distinct from the uninstrumented service. It uses the same
seed for both variants, two generations for every row, and verifies every latest
value after measurement. 1M 8-byte keys and supplemental 100k 32-byte keys each
run three times at 1CPU/512MiB; seed preparation is outside limits at 2CPU/2GiB.
Long-key auxiliary results concern allocation/open, not network-service p99.

Service p99 includes scheduled-arrival queue delay, not just processing. Throughput
at fixed arrival rate is not a capacity ceiling. RSS excludes external client and
host cache. Reopen readiness is sampled at 100ms resolution; it is not precise
engine-only recovery duration. Default adoption requires a separate decision.

Regenerate the allocation figure with `python benchmarks/inline-key/allocation_report.py REPORT_ROOT ALLOCATION_JSON`.
This accepts either local diagnostic results or published allocations.json.
