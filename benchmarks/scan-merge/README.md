# Full scan merge benchmark

Run from the repository root with Rust, Python and matplotlib. Use fresh output directories: builders/runners refuse to overwrite existing work. Allow several GiB for build artifacts and 48 disposable databases. No user library is opened.

```sh
cargo test --locked --features service -- --test-threads=1
cargo test --locked --features service,fixed-log -- --test-threads=1
python benchmarks/scan-merge/build.py artifacts/scan-merge-build
python benchmarks/scan-merge/run.py artifacts/scan-merge-build/binaries.json artifacts/scan-merge-runs
python benchmarks/scan-merge/report.py artifacts/scan-merge-runs artifacts/scan-merge-report
```

The builder archives baseline `d50fe50e02d3377b3078ffa61c37c4951b7f91f1`, adds only `engine_bench.rs` and its test module declaration, and freezes four executables. It requires Git history containing that commit and Cargo dependencies available locally or over the network. Set `CARGO` to select the Cargo executable, and `CARGO_HOME` / `RUSTUP_HOME` if needed. Time builds unset `CATDB_ELISION_ALLOC`; allocation builds set it to 1. Do not run builds or other performance tests alongside this workload.

The runner executes 48 runs: two sizes × two mutation shapes × two implementations × three repeats × two instrumentation modes. Each run has eight full scans plus complete-content, old-snapshot and reopen checks. Mixed uses 10,000 staged operations at both sizes. Durability setup is outside timing. The report keeps all samples and reports trial medians/maxima, not p99 from eight samples. Allocation requests are not live/peak memory; resource reservations are shown separately.

See the [published report](../../docs/benchmarks/scan-merge-2026-11-05/README.md) for all limitations and results. To reproduce the exact candidate later, check out the commit containing that report before invoking the builder.
