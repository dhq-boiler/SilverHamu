# Matched durable-I/O observations

This is a single-writer diagnostic, not a production tracing library. It retains
the existing catdb engine, C ABI, group-commit default and SQLite WAL/FULL settings.
The previous `solo-update` image supplies identical engine binaries and adapters.

## Reproduce

From the repository root, build the base images as documented in
`benchmarks/solo-update/README.md`, then:

```sh
docker build -t catdb-io-phases:local benchmarks/io-phases
python benchmarks/io-phases/test_analyze.py
docker run --rm --network none -e LD_PRELOAD=/usr/local/lib/libiotrace.so -v "${PWD}/benchmarks/io-phases/test_observer.py:/io/test_observer.py:ro" catdb-io-phases:local python /io/test_observer.py
python benchmarks/io-phases/run.py --out artifacts/io-pilot-new --ops 300 --reps 1
python benchmarks/io-phases/analyze.py artifacts/io-pilot-new
python benchmarks/io-phases/run.py --out artifacts/io-main-new
python benchmarks/io-phases/analyze.py artifacts/io-main-new
python benchmarks/io-phases/classify.py artifacts/io-main-new artifacts/io-main-new/classes.json
python benchmarks/io-phases/figures.py artifacts/io-main-new
```

Paths must be new. Docker cleanup is restricted to UUID-named containers and named
volumes created by the runner. Python and matplotlib are needed for figures.
Run the main comparison only after the observer tests and pilot validation pass.
Do not build or benchmark other programs concurrently.

## Exact scope of observation

- Each fixture has 100,000 synthetic rows and 256-byte values, 2 CPU, 4 GiB RAM,
  equal memory/swap limits (no swap allowance), a new Docker named volume, no
  network, and one issuing thread. Three repeats reverse/rotate six conditions:
  catdb, SQLite, SQLite with an initial old reader, each plain/traced.
- A deterministic plan has 8,000 updates; prepare (`begin` plus `put`) and commit
  intervals use the same monotonic clock as the observer. Request timers exclude
  trace activation/deactivation; throughput includes this overhead and appending
  samples. No operation is retried or omitted.
- Native interposition records `write`, `pwrite`/`pwrite64`, `fsync`, `fdatasync`,
  `ftruncate`/`ftruncate64` for already-open `/data` descriptors. Arguments, return
  values and errno are preserved; real synchronization always runs. `write` adds
  a position query before the timed libc body. No trace file is written during
  measured operations. A fixed 200,000-event array bounds tracing memory.
- File descriptors and initial lengths are captured outside timing. Closing a
  tracked descriptor during an active request fails validation. This intentionally
  excludes log rotation/new-file tracing; it is not a universal syscall tracer.
  Direct syscalls, hidden libc calls, read/lock/allocation calls and SQLite's
  internal checkpoint functions are not separately observed. Every request must
  have an observed successful sync; catdb must have exactly one. Reconstructed
  file lengths must match final lengths. These checks detect specific omissions,
  not all theoretically possible missing I/O.
- “Growth” means an observed write extends logical EOF. It does not mean physical
  allocation time was isolated. “Reuse” means no observed write extends EOF and
  no main-database write occurs in that request. WAL offset returning to zero is
  counted separately as an observed rewind. Main-database writes in this WAL
  workload are consistent with checkpointing; no checkpoint-internal CPU timer
  was added. Memory-mapped shared-memory mutations are not observed; explicit shared-memory file writes are included.
- Per-request sync, write, truncate and residual durations form a disjoint
  partition. Prepare/commit are a separate, enclosing partition, never added to
  I/O durations. Nested/overlapping events, invalid IDs, negative/error results,
  short writes, unmatched times, missing per-request sync and unobserved file-size
  changes fail validation. Residual includes uninstrumented DB work, grouping,
  interpreter/wrapper overhead and scheduling; it is not called “CPU time”.
- Sync elapsed includes OS blocking and scheduling; it is not physical-media
  service time. Byte totals are application libc write bytes, not block/media
  amplification. Reusing existing space and the sync call type both differ between
  engines; this study does not independently randomize those mechanisms.

## Integrity and stopping rules

Every fixture checks all initial values, a retained snapshot after an additional
durable update, all latest values, and ordinary close/reopen. The pinned SQLite
condition retains the initial snapshot during timing; primary conditions perform
the old-snapshot check afterwards. Synthetic hashes match across conditions within
each repetition. Counts and source/image identities remain in the raw data.

Five analyzer tests cover exact partition, missing sync, invalid/nested times,
unobserved growth, failed writes and rewind/truncation. The native observer test
checks real call interception, disabled recording, overflow boundaries and close
detection. Engine failure/publication/crash tests from the unchanged base build
remain applicable, but this experiment does not add a new crash or power-cut
certification. Failure to validate observation stops the experiment; adverse
performance and observer overhead remain in the report. No default is changed.
