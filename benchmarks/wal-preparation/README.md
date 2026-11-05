# WAL preparation experiments

The default encoder is retained. Opt-in `wal-direct` removes the intermediate body vector; `wal-prepare` includes it and prepares transaction-owned bytes before serialization under the writer lock. Neither changes log format or sync-before-success. See the [report](../../docs/benchmarks/wal-preparation-2026-11-05/README.md) for the negative timing result and resource/error-order limits.

## Service reproduction

Requires Docker Linux containers, Git, Python and a fresh artifact directory. Build the original comparison client with `docker build -f benchmarks/Dockerfile -t catdb-comparison:local .` if that local image is unavailable. Its Rust library is used only for fixture initialization; timed calls use the common Python TCP client. Preserve the client image ID when comparing runs.

Export baseline `f6b1fa840b5158ebbc31cf3603892633865f5ee7` to a new directory using `git archive`. Then, from the current repository root:

```sh
docker build -f benchmarks/wal-preparation/Dockerfile --build-arg CATDB_FEATURES=service -t catdb-wal-baseline:local /path/to/baseline
docker build -f benchmarks/wal-preparation/Dockerfile --build-arg CATDB_FEATURES=service,wal-direct -t catdb-wal-direct:local .
docker build -f benchmarks/wal-preparation/Dockerfile --build-arg CATDB_FEATURES=service,wal-prepare -t catdb-wal-prepare:local .
python benchmarks/wal-preparation/run_service.py --variant baseline=catdb-wal-baseline:local --variant direct=catdb-wal-direct:local --variant prepare=catdb-wal-prepare:local --single-variants baseline direct prepare --clients 1 4 --workloads update --ops 8000 --reps 3 --out artifacts/wal-service-runs
```

Use an absolute Dockerfile path for the baseline build if your Docker client requires it. The runner reuses `single-request/client.py` and `runner.py`, creates uniquely named disposable networks/volumes/containers and removes only its own resources. No user database is opened. All variants use the same client image, limits, operation generator and content oracle. Do not run unrelated builds/tests during measurement.

## Auxiliary engine reproduction

Requires Rust, Python and matplotlib. The output directories must be new.

```sh
python benchmarks/wal-preparation/build_aux.py artifacts/wal-aux-build
python benchmarks/wal-preparation/run_aux.py artifacts/wal-aux-build/binaries.json artifacts/wal-aux-runs
python benchmarks/wal-preparation/report.py artifacts/wal-service-runs artifacts/wal-aux-runs artifacts/wal-report
```

Set `CARGO`, `CARGO_HOME`, `RUSTUP_HOME` if necessary. The builder freezes three executables with `lock-timing` and allocation counting enabled, using legacy/direct/prepared modes in the same candidate source. Keep these diagnostics separate from the primary uninstrumented Docker measurement. On the recorded run these auxiliary executables ran on Windows; Docker service results ran on Linux. They are not directly comparable performance cohorts.

Each auxiliary trial seeds 100k rows, pins an old snapshot, runs four barrier-synchronized writers for 512 attempts, and checks old/current/reopened data. Conflict tests deliberately reject three of four hot-key attempts and do not retry. The encoding probe has 128 samples and a three-byte key. Raw totals are cumulative allocator requests, not RSS. To reproduce the exact candidate later, check out the commit containing this report first.
