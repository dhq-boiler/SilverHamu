# Refreshed product comparison

This reuses the original seeded operation generator, client threads, SQL adapters,
durability settings and full-content/restart checks. Only catdb's timed service
adapter changes when `--single-request` is supplied: get_latest, scan_latest and
put_commit complete one operation in one protocol request. Initialization and
verification deliberately continue through the explicit transaction API. Each
scan_latest is a new snapshot; this is not a replacement for a multi-page snapshot.

No engine defaults change. The current default inline-latest feature is enabled;
wire-buffer, inline-key and diagnostic features are disabled. Each server and each
client/embedded container gets 2 CPUs and 4GiB. This is separate from the 1-CPU,
512MiB capacity experiments. SQL Server is excluded pending publication permission.

```sh
docker build -f benchmarks/Dockerfile -t catdb-peer:local .
python benchmarks/run.py --out artifacts/peer-smoke --image catdb-peer:local --single-request --rows 1000 --ops 100 --reps 1
python benchmarks/run.py --out artifacts/peer-new/main --image catdb-peer:local --single-request --rows 100000 --ops 8000 --reps 3
python benchmarks/run.py --out artifacts/peer-new/legacy --image catdb-peer:local --engines catdb-service --rows 100000 --ops 8000 --reps 3
python benchmarks/peer-refresh/report.py artifacts/peer-new
```

Pre-existing product images are required as documented in ../README.md. Preserve
the recorded digests for exact version reproduction. The legacy supplement runs
after the product comparison; it is not interleaved with the main catdb samples,
so environmental drift can contribute to the difference. Both use exactly the
same service and embedded binaries. No response-time observation is treated as an
engine-only measurement or proof of a specific internal bottleneck.

Verification perspectives:
- EQ: every final key/value compared with the deterministic update stamps across
  all products and both catdb API paths; no silent retries or no-op updates.
- BV: empty initial database, missing key, empty range and last-record range.
- ST: updates visible after commit, full checks after each case and reopen.
- ER: failures are retained and stop the run; failed conditions are not plotted as
  successes. Ordinary restart checks are not power-loss or crash-injection tests.
- Measurement: 3 repetitions, 1/4 clients, 8000 operations/case, all latency samples;
  source/image identities, exact percentile/throughput recomputation, per-product
  version/config stability and independent cross-product content equality.

The default runner still uses the legacy adapter so historical reproduction does
not silently select a new protocol. Existing historical source hashes require the
corresponding historical checkout, as before.
