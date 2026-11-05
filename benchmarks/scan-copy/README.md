# Full-scan copies and output construction

Use fresh output directories. Requires Rust, Python, matplotlib and Git history containing `6fbe8ff5cdeb0095ca2a22a48d0aa603f9698a80`. Allow several GiB for builds, 48 disposable transaction databases and 384 exported files. No user data is opened.

```sh
cargo test --locked --features service -- --test-threads=1
cargo test --locked --features service,fixed-log -- --test-threads=1
python benchmarks/scan-copy/build.py artifacts/scan-copy-build
python benchmarks/scan-copy/run.py artifacts/scan-copy-build/binaries.json artifacts/scan-copy-runs
python benchmarks/scan-copy/report.py artifacts/scan-copy-runs artifacts/scan-copy-report
```

The builder adds the common benchmark to the archived baseline and replaces only the marked adapter block using `baseline_adapter.rs.txt`. The baseline's production code is unchanged. Time builds unset `CATDB_ELISION_ALLOC`; separate allocation builds set it to 1. All four executables are frozen before running. Set `CARGO`, `CARGO_HOME`, `RUSTUP_HOME` if needed; dependencies must be cached or downloadable. Do not build or run other tests concurrently with timing.

The runner performs 48 runs: two sizes × two shapes × two versions × three trials × two instrumentation modes. Each run measures eight owned scans, eight frame constructions, eight shared-result constructions and eight actual durable exports. The shared result differs from the owned return type and is plotted separately. Framing is an internal construction measurement excluding DLL dispatch and caller-buffer copies. Exports always read latest committed values, excluding staged edits in either shape. Every result and export is validated, along with old snapshots and reopening.

Read the [full report](../../docs/benchmarks/scan-copy-2026-11-05/README.md) before interpreting allocation totals, reservation lifetimes or timing differences. To reproduce the exact candidate later, check out the commit containing that report.
