# Validation

- Storage-path guards: 2 passed. Existing directory contents survive rejection;
  bind-root/mode mismatch is rejected before creating the output folder.
- Existing resource client: 9 passed, including bounded queues, old values,
  readiness gate and budget-refusal recovery.
- Shared timing/identity regression checks: 10 passed (7 + 3).
- [Local test output](tests.log.gz): 21 tests passed, zero failures.
- Two 1,000-row pilots: sync/readback, exclusive DB ownership, full values,
  old versions, budget refusal/rollback recovery and kill/reopen passed.
- Six 1M-row main trials: 96,000 requests, zero refusals/errors, 30 full-data
  checks, six identical seed hashes, 9,600 old-version update checks and six
  forced DB-child termination/reopen verifications passed.
- The actual DB mounts matched volume/bind mode; control storage was a named
  volume in every case. 1 CPU / 512 MiB / zero swap limits were verified at
  both startup and reopening. No experiment containers or Docker volumes remained.
- The same engine binary was used throughout; no Rust engine/API code changed.
  The commit's CI runs the existing Windows/Linux safety and API matrix and Docker
  checks. Results are linked from GitHub commit history.
- No real power cut, hardware fault, or claim of identical physical durability.

The C: bind fixtures remain only in the new synthetic-data working directory
`C:/Git/NewDBMS/artifacts/storage-path-data-2026-11-06`; they were not committed.
