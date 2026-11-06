# Validation

- Measured image: existing backlog/service protocol suite, 33 passed.
- Measured image: deterministic staged-budget boundary, rollback and restart test,
  1 passed. No resource limit increased.
- New client tests: 5 passed (mixed values/old snapshot, queued-plus-active credits,
  duplicate-key carry, resource refusal and disconnected request handling).
- New analysis tests: 3 passed (completion-based recovery, no-backlog boundary,
  failed screening prevents extension).
- Four verified workloads: 16,000 offered, 15,995 completed, five explicitly
  resource-refused, zero queue-admission refusals, zero other operation errors.
  20 full-content checks, four seeds, 1,595 accepted-update old-version checks,
  four budget rollback/recovery probes and four kill/reopen checks passed.
- Initial path setup failure and partial resource-refused run are separately
  preserved and excluded from full-verification counts.
- Dedicated experiment containers/volumes were absent after cleanup.
- CI on the final commit runs the Windows/Linux safety/API matrix and Docker.
  Link via GitHub commit history. No real power-cut test was performed.

Logs: [protocol](protocol.log.gz), [budget boundary](budget-test.log.gz).
