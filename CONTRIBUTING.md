# Contributing

silverhamu is experimental. Keep changes small and document their effect on durability, compatibility, and resource usage. Use synthetic fixtures and never commit personal databases, tokens, crash dumps, or workstation-specific paths.

## Local checks

Use Rust 1.89 or newer and Python 3. Build the service before its process tests:

```sh
cargo test --locked --features service -- --test-threads=1
cargo build --locked --features service --bin silverhamu-server
python tests/service_protocol.py
cargo build --locked --release --lib
python -m unittest discover -s tests -p "*ffi_checks.py"
cargo clippy --locked --features service --lib --bin silverhamu-server -- -D warnings
```

Set `CATDB_SERVER` or `NEWDBMS_TEST_DLL` to use an alternate executable/library. Defaults select the platform's files in `target/debug` and `target/release`. The FFI test loader supports Windows, Linux and macOS filenames; the current CI runs Windows and Linux.

Core recovery tests create child processes and temporary files. Windows retirement tests need a filesystem and permissions that allow hard links. Network tests need loopback connections. A permission-denied test run is not a passing result; diagnose the environment instead of weakening the assertions.

Some ignored Rust tests are child-process entry points or larger benchmarks. Parent tests run the required crash workers. Do not indiscriminately run every ignored test without reading its preconditions.

Container verification requires Docker:

```sh
docker build -t silverhamu:local .
python tests/service_container.py
```

Tests create their own data under ignored `artifacts/`. Container tests remove only the uniquely named resources they created. Do not substitute a real database for a fixture.

## Compatibility and scope

The Cargo package, Rust library and native library are named `silverhamu`. The C# bindings retain the `NewDbms` namespace, and C ABI symbols and calling conventions are unchanged. Rust consumers must update imports from `newdbms` to `silverhamu`; native consumers must load the renamed library. File-format or ABI changes require an explicit compatibility plan.

The standalone extraction excludes the original application integration and private experiment logs. Benchmark claims need a reproducible workload, hardware description, correctness checks, and both throughput and latency distributions. Avoid claiming universal superiority from a single workload.

Report bugs with a minimal synthetic reproducer, platform/toolchain, and sanitized diagnostics. Never include credentials or private database contents in a public issue.

Run test cases sequentially to isolate subprocess crash fixtures from other
tests' file-lock lifetimes. Tests still exercise their own concurrent readers
and writers; this flag does not disable that internal concurrency.

## Comparing an optimization with other databases

For each measured performance improvement, update the README with a fresh peer comparison figure for the affected workload alongside the silverhamu before/after result. Keep the key stream, operation count, client count, durability, CPU and memory limits comparable. Report versions, individual trials, verification and raw data. Group embedded and service products by execution mode. When an experimental path is reachable only through a native test harness, measure that actual path, identify its driver, and label the shipped service separately. Record unavailable products and publication restrictions with the results. Include slower outcomes.
