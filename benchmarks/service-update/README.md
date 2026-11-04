# Service update follow-up

This experiment changes group-commit coordination, not the operations in
`benchmarks/runner.py`. It reuses that client and its full-content verifier.
The frozen baseline is the engine at `1c22601` (same runtime sources as the
original product comparison); the candidate enables `group-handoff` whenever
`service` is built. Embedded builds without `service` retain their feature defaults.

## Reproduce the paired comparison

Run from the repository root. Docker is required. Use fresh output directories;
the runner refuses to overwrite an existing result directory. The examples use
a POSIX shell; PowerShell accepts the same commands without the line continuations.

```sh
git worktree add --detach artifacts/service-update-baseline 1c22601
docker build -f artifacts/service-update-baseline/benchmarks/Dockerfile -t catdb-comparison:local artifacts/service-update-baseline
docker build -f benchmarks/service-update/Dockerfile -t catdb-update-handoff:local .
python benchmarks/service-update/run.py --variant baseline=catdb-comparison:local --variant handoff=catdb-update-handoff:local --out artifacts/service-update-rerun --reps 3
python benchmarks/service-update/validate.py artifacts/service-update-rerun
docker run --rm -v "$PWD:/repo" -w /repo catdb-comparison:local python benchmarks/service-update/plot.py artifacts/service-update-rerun
```

Do not overwrite the frozen baseline image while an experiment is running.
Images, the original Python 3.12 client, named volumes and a private Docker
network are local to the experiment. Each server and client has a 2-CPU quota and
4 GiB limit. The data set has 100,000 records, 8-byte keys and 256-byte values.
There are 8,000 operations per condition: get, 100-row range and single-row
update, each using its own transaction, at 1 and 4 clients. Repeats use the
original shuffled case order; variant order reverses on even repeats.

The runner verifies all values and empty/end boundaries after every case and
after a server restart. Failures stop the run and are retained; uncertain
commits are not automatically retried. Verification, initialization and restart
are outside timed operations. Read/write phases are separate, warm-cache and
closed-loop, with no concurrent scheduled checkpoint. This is not a production
mixed-load or open-loop tail-latency benchmark.

## Diagnose stages and measure observer overhead

`lock-timing` plus `CATDB_DIAGNOSTICS=1` adds timing fields to commit responses.
Normal service builds do not include these fields or the extra timing code.
No keys, values, tokens or passwords are recorded in profiles. For a diagnostic
baseline, apply only the saved instrumentation patch to the detached baseline:

```sh
git -C artifacts/service-update-baseline apply "$PWD/benchmarks/service-update/diagnostics.patch"
docker build -f "$PWD/benchmarks/service-update/Dockerfile" --build-arg CATDB_FEATURES=service,lock-timing -t catdb-update-baseline-diag:local artifacts/service-update-baseline
docker build -f benchmarks/service-update/Dockerfile --build-arg CATDB_FEATURES=service,lock-timing -t catdb-update-handoff-diag:local .
python benchmarks/service-update/run.py --variant baseline=catdb-comparison:local --variant baseline-diag=catdb-update-baseline-diag:local --variant handoff=catdb-update-handoff:local --variant handoff-diag=catdb-update-handoff-diag:local --profile-variants baseline-diag handoff-diag --clients 4 --workloads update --reps 3 --out artifacts/service-update-overhead
python benchmarks/service-update/validate.py artifacts/service-update-overhead
python benchmarks/service-update/analyze.py artifacts/service-update-overhead
```

The overhead comparison includes server timing, response serialization and
client sample collection together. It cannot attribute their costs separately.
The main performance comparison has all this instrumentation disabled.

Trace boundaries are submit, group selection, writer-lock request, lock acquired,
log-write start/end, sync end and publication end. `batch_queue` includes the
handoff coordinator wait. `writer_wait` is the engine's writer lock after group
selection. Group membership is counted using the shared sync-end timestamp.
`client_extra` includes transport, parsing and scheduling around server dispatch;
it is not a measurement of network latency alone. Stage means are additive;
stage p99 values are not additive. The chart reports complete operation latency
(begin + put + commit), whereas trace stages describe the commit dispatch.

## Safety checks

```sh
cargo test --locked --features service -- --test-threads=1
cargo clippy --locked --features service --lib --bin catdb-server -- -D warnings
cargo build --locked --features service --bin catdb-server
python tests/service_protocol.py
cargo build --locked --release --lib
python -m unittest discover -s tests -p "*ffi_checks.py"
```

The deterministic sync gate test holds persistence while three followers queue:
no response or new value may appear, then four updates must use two syncs and
survive reopen while the old snapshot stays unchanged. Existing tests cover
31/32/33 requests, the 4 MiB byte boundary, lone requests, write/read/range
conflicts, failed synchronization, and subprocess exits before/after sync.
Process exits are not real power-loss or device-failure tests.
