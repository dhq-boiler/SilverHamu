# Validation

- Local Linux Rust `service,lock-timing,benchmark-metrics`: **146 passed, 0 failed,
  12 ignored**. Ignored cases are existing opt-in performance fixtures, not failures
  hidden by this experiment. See [rust.log.gz](rust.log.gz).
- The measured image itself: **35 C ABI tests**, **23 service protocol tests**,
  all passed ([ffi.log](ffi.log), [service.log](service.log)).
- Observer partition/invalid-event tests: **7 passed**; namespace/thread identity:
  **3 passed**; resource-envelope client and readiness gate: **9 passed**.
- Six main runs: 96,000 requests; 30 full-content checks; six matching seed checks;
  9,600 held-old-generation update checks; six staged-limit recovery probes;
  six forced child termination/reopen checks. All passed.
- Main capture: exact sync-counter matches (1,600 / 1,595 / 1,599), zero recorded
  ring losses, valid bounds, and private-instance cleanup. An independent final
  tracefs inventory found no remaining `catdb-aged-sync-*` instance; no experiment
  containers remained after tests (the unrelated existing build container was kept).
- Initial invalid identity capture was retained, not counted as a valid observation.
  No simulated physical power failure or hardware fault was performed.
- Windows/Linux CI results are available on the commit linked from GitHub history.

Commands:

```text
python benchmarks/aged-sync/test_analysis.py
python benchmarks/aged-sync/test_identity.py
python benchmarks/resource-envelope/test_client.py
cargo test --locked --offline --features service,lock-timing,benchmark-metrics -- --test-threads=1
NEWDBMS_TEST_DLL=/usr/local/lib/libnewdbms.so python -m unittest discover -s tests -p '*ffi_checks.py'
CATDB_SERVER=/usr/local/bin/catdb-server python tests/service_protocol.py
```

Python API commands ran in the same measured image. Rust tests used its matching
builder source with tests/benchmarks mounted, outside the performance intervals.
