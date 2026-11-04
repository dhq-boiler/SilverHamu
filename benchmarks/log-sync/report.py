"""Generate the report from validated, contemporaneous samples."""
import gzip, json, pathlib, statistics, sys
root = pathlib.Path(sys.argv[1])
rows = json.loads((root/'results.json').read_text(encoding='utf-8'))
observer = json.loads((root/'observer/results.json').read_text(encoding='utf-8'))
diagnosis = json.loads((root/'observer/diagnosis.json').read_text(encoding='utf-8'))
def med(engine, clients, metric):
    values = [r[metric] for r in rows if r['engine']==engine and r['clients']==clients]
    assert len(values)==3
    return statistics.median(values)
def omed(variant, metric):
    return statistics.median(r[metric] for r in observer if r['variant']==variant)
before = med('catdb-current',4,'ops_s'); after = med('catdb-candidate',4,'ops_s')
bp = med('catdb-current',4,'p99_ms'); ap = med('catdb-candidate',4,'p99_ms')
text = f'''# Log synchronization and service grouping follow-up

The selected change improves **amortization of durable synchronization**, not the
latency of an individual synchronization. No file format, `sync_all` call,
conflict check or acknowledgement ordering was changed.

In three contemporaneous runs, four-client throughput rose from **{before:.1f}
to {after:.1f} operations/s ({(after/before-1)*100:.1f}% higher)** and median
run p99 fell from **{bp:.3f} to {ap:.3f} ms ({(1-ap/bp)*100:.1f}% lower)**.
All plots include slow and fast runs. These are warm-cache, closed-loop Python
API measurements, not general DBMS rankings or engine-only ceilings.

![Durable service update throughput and p99](service-updates.svg)

'''
text += (root/'table.md').read_text(encoding='utf-8')
text += '\n## Gap to peers\n\n'
for peer in ('postgres','oracle'):
    rate=med(peer,4,'ops_s')
    text += f'- {peer}: catdb reaches {after/rate*100:.1f}% of its update throughput, up from {before/rate*100:.1f}%. The absolute throughput gap shrinks by {(after-before)/(rate-before)*100:.1f}%.\n'
text += '''
PostgreSQL and Oracle Free remain faster here. They were rerun in the same
experiment; comparisons do not mix this run with the older workstation results.
The single-client panel exposes the cost when grouping is unavailable.

## Why the original sync cost remains

The append probes compare a 32 MiB file and 512 writes of 600 bytes per run,
three shuffled repeats. Numbers below are the median of each run's mean and p99;
they are file I/O diagnostics, not database throughput measurements.

'''
for filename, prefix in [('append-and-fixed.json.gz','sync'),('durable-write.json.gz','durable')]:
    with gzip.open(root/'probes'/filename,'rt') as f: probe=json.load(f)
    text += '| Probe ('+prefix+' time) | Mean ms | p99 ms |\n|---|---:|---:|\n'
    for case in sorted({c['case'] for c in probe['cases']}):
        selected=[c for c in probe['cases'] if c['case']==case]
        assert len(selected)==3
        text += f"| {case} | {statistics.median(c[prefix+'_mean_ms'] for c in selected):.3f} | {statistics.median(c[prefix+'_p99_ms'] for c in selected):.3f} |\n"
    text += '\n'
text += '''The raw [append/fixed-size probe](probes/append-and-fixed.json.gz) and
[durable-write probe](probes/durable-write.json.gz) retain every observation.
Append fsync/fdatasync, keeping allocation beyond EOF, combined writes,
O_SYNC/O_DSYNC and RWF_DSYNC all remained near 3.3–3.4 ms per durable operation.
Fixed-size overwrite with fdatasync was near 1.6 ms, but it changes the file's
logical-length model. Adopting it requires a separate recovery protocol and is
**not part of this change**. It is evidence consistent with file growth metadata
contributing to the cost, not a definitive kernel-level attribution.

[Linux fsync/fdatasync semantics](https://github.com/mkerrisk/man-pages/blob/master/man2/fsync.2)
require metadata needed to retrieve data, including changed file length, to be
synchronized even with fdatasync.
[KEEP_SIZE preallocation](https://man7.org/linux/man-pages/man2/fallocate.2.html)
keeps logical EOF unchanged, so it cannot remove that requirement.
The implementation retains Rust's
[`File::sync_all`](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all).

## Selected policy and rejected candidates

The selected service policy allows a recently full group to re-form within one
1 ms collection budget; four queued requests end collection early. A group that
actually waited behind a prior writer can also refill an expired partial batch.
The two extra budgets do not accumulate. The recent-full hint expires after 2 ms
and uses one fixed-size entry per database. Idle/small-group operation retains the
existing 200 us collection budget. A sudden four-to-one transition inside that
2 ms interval can incur one extra wait; OS scheduling may exceed the requested
wait, so this is not a real-time guarantee. Queue count/byte limits are unchanged.

This follows the general principle that arrivals during one durable flush can
form the next group, discussed in
[PostgreSQL's WAL configuration documentation](https://www.postgresql.org/docs/18/wal-configuration.html).
No external database source code was copied.

Rejected exploratory pilots are retained rather than omitted:

- [Partial-group-only refill](pilots/partial-only/results.json): 570.5 to 621.8
  operations/s, but p99 worsened from 10.463 to 11.118 ms at four clients.
- [Actual-handoff-only refill](pilots/handoff-only/results.json): instrumented
  567.8 to 603.9 operations/s, but p99 worsened from 9.808 to 10.646 ms.
- [Recent-full-group pilot](pilots/recent-full-group/results.json): instrumented
  748.7 operations/s and 7.596 ms p99; selected for the full comparison.

These are one-repeat exploratory runs with different policies, not evidence of
statistical significance. Their image IDs, configurations and raw distributions
remain in each pilot directory. They do not all correspond to the final source.

## Diagnostic overhead and synchronization breakdown

Separate four-client runs alternate ordinary and instrumented builds, three
repeats each. Instrumentation includes server timestamps, response encoding and
client collection. It can change arrival timing; a higher measured throughput
with instrumentation does not imply negative overhead. The main plot has it off.

| Build | Operations/s | p99 ms |
|---|---:|---:|
'''
for v in ('current','current-diag','candidate','candidate-diag'):
    text+=f'| {v} | {omed(v,"ops_s"):.1f} | {omed(v,"p99_ms"):.3f} |\n'
text+='\n| Instrumented build | Updates/sync | Mean sync ms | Sync p99 ms |\n|---|---:|---:|---:|\n'
for v in ('current-diag','candidate-diag'):
    ds=[d for d in diagnosis if d['variant']==v]
    assert len(ds)==3
    text+=f'| {v} | {statistics.median(d["commits_per_sync"] for d in ds):.3f} | {statistics.median(d["stages"]["sync"]["mean_ms"] for d in ds):.3f} | {statistics.median(d["stages"]["sync"]["p99_ms"] for d in ds):.3f} |\n'
text+='''
Stage statistics are per commit (a shared sync appears for every member), with
medians across runs. Stage p99s cannot be added to reconstruct response p99.
See [all stage distributions](observer/diagnosis.json), [observer raw results](observer/results.json)
and [methodology/reproduction](../../../benchmarks/log-sync/README.md).

## Verification, provenance and limitations

- Main run: 192,000 attempted/completed operations, zero errors/retries, 48
  full-content/boundary checks and 12 restarts. Cross-engine hashes agree.
- Observer run: 96,000 operations, 36 full-content checks and 12 restarts.
  All commit traces are present, monotonic and within group limits.
- Selected-policy Linux Rust suite: 101 passed, four worker helpers ignored;
  service protocol: 13 passed; C ABI: 30 passed; clippy passed.
- Existing tests cover old snapshots, conflicts, resource limits, failed writes,
  incomplete logs and subprocess termination around synchronization/publication.
  Policy boundary tests cover empty/small/full groups and hint expiry. A gated
  sync test proves no early successful response or uncommitted visibility.
- [Implementation CI](https://github.com/dhq-boiler/catdb/actions/runs/41890144568)
  passed on Windows and Linux, including service/C ABI checks and the Docker
  restart test.
- Process termination and fault injection are not real power-loss testing.
  Durability still depends on the OS and device honoring synchronization.
- This workload has no concurrent checkpoint, mixed reads, replication, cold
  cache, open-loop arrivals or larger-than-memory data. Results from three runs
  describe this machine and workload; they are not confidence intervals.

[Environment and image digests](environment.json), [raw run summaries](results.json),
[all numerical results](table.md) and [source/build provenance](provenance.json)
are published with the original compressed response-time samples. Earlier
comparisons remain unchanged.
'''
(root/'README.md').write_text(text,encoding='utf-8')
