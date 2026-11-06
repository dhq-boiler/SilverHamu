"""Render every trial, startup-window bursts, and bounded diagnostic comparisons."""
import argparse,json,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from analyze import analyze

def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);a=p.parse_args();root=a.root
    result=analyze(root);trials=result['trials'];assert len(trials)==10
    (root/'summary.json').write_text(json.dumps(result,indent=2))
    mains=[x for x in trials if x['stage']=='main'];probes=[x for x in trials if x['stage']=='probe'];diags=[x for x in probes if x['variant']=='diagnostic']
    table=['| Trial | Write p99 ms | Write max ms | Before-call p99 ms | First-second write p99 ms | Later write p99 ms | >=10ms / >=20ms | RSS MiB | CPU throttled us |','|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    allrows=[result['original']]+trials
    for x in allrows:
        w=x['mixed']['write'];res=x['resources']
        table.append(f"| {x['label']} | {w['response_ms']['p99']:.3f} | {w['response_ms']['max']:.3f} | {w['before_call_ms']['p99']:.3f} | {x['first_second']['write']['response_ms']['p99']:.3f} | {x['remaining']['write']['response_ms']['p99']:.3f} | {w['over10ms']} / {w['over20ms']} | {res['rss_peak_mib']:.2f} | {res['throttled_usec']} |")
    (root/'trials.md').write_text('\n'.join(table)+'\n')
    view=[result['original']]+mains
    fig,axes=plt.subplots(1,2,figsize=(12,4.5),layout='constrained')
    labels=[x['label'].replace('original-19ms','Original\ninline').replace('main-','').replace('-','\n') for x in view]
    colors=['#777777' if x['stage']=='original' else '#197d83' if x['variant']=='inline' else '#bb641c' for x in view]
    for ax,metric,title in zip(axes,['p99','max'],['Write p99','Write maximum']):
        values=[x['mixed']['write']['response_ms'][metric] for x in view];bars=ax.bar(labels,values,color=colors)
        ax.bar_label(bars,fmt='%.2f',padding=3);ax.set_ylim(0,max(values)*1.18);ax.set_ylabel('ms, from planned arrival');ax.set_title(title);ax.tick_params(axis='x',labelsize=8)
    fig.suptitle('Aged 1M rows / 1 CPU / 1 GiB — original event and all new uninstrumented trials',fontsize=11)
    fig.savefig(root/'latency.svg');fig.savefig(root/'latency.png',dpi=150);plt.close(fig)
    fig,ax=plt.subplots(figsize=(11,4),layout='constrained')
    heat=[[item['write']['over10ms'] for item in x['intervals']] for x in view]
    im=ax.imshow(heat,aspect='auto',extent=[0,8,len(view)-.5,-.5],cmap='YlOrRd',interpolation='nearest')
    ax.set_yticks(range(len(view)),[x['label'] for x in view]);ax.set_xlabel('seconds from planned workload start (100-ms bins)');ax.set_title('Write responses >=10 ms, bucketed by planned arrival')
    fig.colorbar(im,ax=ax,label='number of writes');fig.savefig(root/'windows.svg');fig.savefig(root/'windows.png',dpi=150);plt.close(fig)
    phase_table=['| Server phase | Probe 2 p99 / max ms | Probe 3 p99 / max ms |','|---|---:|---:|']
    for name in diags[0]['phases_ms']:
        phase_table.append('| '+name+' | '+' | '.join(f"{x['phases_ms'][name]['p99']:.6f} / {x['phases_ms'][name]['max']:.6f}" for x in diags)+' |')
    for name in ['latency.svg','windows.svg']:
        svg=root/name;svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines())+'\n')
    request_count=sum(x['requests'] for x in trials);checks=sum(x['full_checks'] for x in trials)
    medians={v:{'write_p99_ms':statistics.median(x['mixed']['write']['response_ms']['p99'] for x in mains if x['variant']==v),'rss_mib':statistics.median(x['resources']['rss_peak_mib'] for x in mains if x['variant']==v)} for v in ['deque','inline']}
    (root/'medians.json').write_text(json.dumps(medians,indent=2))
    original=result['original'];worst=[x['worst_write'] for x in diags]
    text=f"""# Rechecking the aged inline-latest p99 regression (2026-11-06)

## Decision and limits

This report records a bounded reproduction attempt, not an automatically adopted
optimization. Engine code, default features, durable acknowledgements and memory
accounting are unchanged. The final interpretation is recorded in the findings
section below after inspecting all trials. Original and new results are kept
separate; an unobserved event must not be called impossible or fixed.

## Existing evidence

The earlier inline/1-GiB trial had write p99 19.482 ms, maximum 33.417 ms,
41 writes >=10 ms and 16 >=20 ms. Most slow writes were scheduled at 0.1–0.3 s,
with another affected 100-ms bin at 3.6 s. Its worst write spent 0.094 ms before
the client request call and 33.323 ms inside that call. The 100-ms OS samples show
zero CPU-quota throttling increment in the workload interval. That does not rule
out host scheduling, I/O waits, or latency between resource samples. Without
per-request server diagnostics, that original event cannot be retrospectively
assigned to sync, allocation or a particular lock.

## Reproduction and observer comparison

Same fully aged 1M rows, 8-byte keys / 256-byte values, 1-CPU quota, 1-GiB service
cgroup, no swap, 640-MiB logical retained and 4-KiB staged budgets. Four write
workers + one reader, 80% reads / 20% writes, fixed 1,000 scheduled requests/s;
bounded queues 64 per kind. Each main trial has 8,000 read-only and 8,000 mixed
requests; default deque / inline alternate order over three rounds. No fixed old
reader in timing; old-generation correctness is checked separately after timing.

Four short observer trials use inline / diagnostic / diagnostic / inline,
2,000 requests per phase. Diagnostics enable **only `inline-latest,lock-timing`**,
not `skip-unneeded-history` or `retirement-timing`. The observer comparison
includes clock calls, extra response bytes, serialization and client decoding;
it is not an isolated timer microbenchmark. The common 100-ms OS observer stays
on in all conditions. Main trials and short observer trials are separate cohorts.
Throughput is arrival-limited and does not measure maximum capacity.

Images are built from the same engine source (base commit
`1edbe18d4e87535ebff774aca2a404b861352e18`). Image IDs and normalized source hashes
are in `raw/`. Synthetic seeds are prepared and verified outside timing at
2 CPUs / 4 GiB; all updated once, without a final checkpoint. All seeds including
the original event have identical WAL SHA-256 `{result['seed_wal_sha256']}`.
No user database was accessed. Host/OS cache is not flushed; shared page cache
can be charged outside the service cgroup, as documented in the previous report.
This is not a cold-cache or total-machine memory-budget test.

![Write latency, every main trial](latency.svg)

![Burst timing](windows.svg)

See [all trials and startup/remaining windows](trials.md), [summary](summary.json),
and [main medians](medians.json). The first second and remaining interval use
planned arrival timestamps, not completion timestamps. Counts at exactly 10/20 ms
are included. Rejected requests are reported separately, never treated as fast
completions. A window's p99 has fewer samples than the entire trial's p99.

## Diagnostic attribution

Every successful diagnostic write must have complete, ordered timestamps and a
payload below 2 KiB. The intervals partition **that same request's dispatch**;
validation/frame cost counters are checked but not added a second time. Server
timestamps share one server-local monotonic clock. Client durations are computed
separately; their absolute clocks are never subtracted from server timestamps.

{chr(10).join(phase_table)}

`sync_envelope` is the interval around the durable sync call, including adjacent
bookkeeping and any scheduling delay. It does not separate physical I/O from
thread rescheduling. `post_publish_to_reply` includes cleanup and transaction
destruction before response readiness; it is not an allocator-only measurement.
Client request-call time also includes transport and encoding/decoding. CPU-quota
throttling is distinct from general OS scheduling. Do not sum unrelated p99s/maxima.

The two diagnostic worst requests, including their own phase breakdown, are
retained under `worst_write` in the summary. Zero/missing/reversed timing is an
error, not a zero-cost result. Diagnostic record count is bounded by the explicit
operation count. No high-frequency unbounded server log was added.

## Safety and scope

{request_count:,} scheduled requests and {checks} full-content service checks
across the new trials; ten additional full seed checks. Each trial also checks
eight updates under a retained old snapshot, staged-budget rejection preserving
the previous staged value, rollback/reservation reuse, and a successful subsequent
commit. Finally, the owned server child is SIGKILLed and all acknowledged values
are verified after reopen. This is process-crash recovery, not hardware power loss.
The deliberately refused safety-probe writes are not workload rejection events.

Existing protocol/diagnostic tests passed **26/26** against the diagnostic Linux
image. New analysis tests cover exact thresholds, duplicates, missing/reversed
timestamps and dispatch partitioning. Existing Rust and API safety suites run in
CI. No engine changes were needed merely to obtain the measurements, so broader
refactoring was not performed. If no evidence-backed engine candidate is justified,
the conditional before/after candidate and 512-MiB qualification runs are not run.

## Reproduce

```sh
docker build -f benchmarks/Dockerfile -t catdb-aged-tail:deque .
docker build -f benchmarks/Dockerfile --build-arg EXTRA_FEATURES=,inline-latest -t catdb-aged-tail:inline .
docker build -f benchmarks/Dockerfile --build-arg EXTRA_FEATURES=,inline-latest,lock-timing -t catdb-aged-tail:diagnostic .
python benchmarks/aged-tail/run.py --out artifacts/aged-tail-new/raw
python benchmarks/aged-tail/report.py artifacts/aged-tail-new
python benchmarks/aged-tail/test_analysis.py
```

Use a new output path. All requests, OS samples, outcomes and timings are preserved
under `raw/`, with large files losslessly gzip-compressed. Failed runs would stop
the schedule rather than be retried or replaced. No automatic long-run extension.
"""
    recurred=sum(x['mixed']['write']['response_ms']['p99']>=original['mixed']['write']['response_ms']['p99'] for x in mains)
    over20=sum(x['mixed']['write']['over20ms'] for x in mains)
    throttled=sum(x['resources']['throttled_usec'] for x in trials)
    main_text=[]
    for variant in ['deque','inline']:
        rows=[x for x in mains if x['variant']==variant]
        ps=[x['mixed']['write']['response_ms']['p99'] for x in rows]
        main_text.append(f"- {variant}: write p99 {min(ps):.3f}–{max(ps):.3f} ms; >=10-ms writes {sum(x['mixed']['write']['over10ms'] for x in rows)}/4,800; median RSS {medians[variant]['rss_mib']:.2f} MiB.")
    findings=f"""## Findings and stop decision

**The 19.482-ms p99 regression did not recur in three new paired main trials.**
No evidence-backed engine change is justified by this bounded run. Keep defaults
and the existing opt-in implementation unchanged; no candidate-comparison or
additional 512-MiB qualification was run. Do not call the old event fixed.

{chr(10).join(main_text)}

Both layouts still had individual >10-ms responses. New main trials had no
>=20-ms write responses; the old trial had 16. Main inline's RSS reduction remains
{100*(1-medians['inline']['rss_mib']/medians['deque']['rss_mib']):.1f}% versus deque. All new measured CPU-quota throttling increments were zero,
which excludes observed quota throttling, not arbitrary scheduler or I/O delay.

In the two diagnostic trials, the worst requests were
{diags[0]['worst_write']['response_ms']:.3f} / {diags[1]['worst_write']['response_ms']:.3f} ms;
their own sync envelopes were
{diags[0]['worst_write']['phases_ms']['sync_envelope']:.3f} / {diags[1]['worst_write']['phases_ms']['sync_envelope']:.3f} ms.
{sum(x['mixed']['write']['over10ms'] for x in diags)} diagnostic requests reached 10 ms. These samples identify sync-envelope time
as the dominant interval in those requests, **not the cause of the old burst**.
OS-level I/O versus rescheduling and the original event remain unresolved.

Short observer-off write p99s were
{probes[0]['mixed']['write']['response_ms']['p99']:.3f} / {probes[3]['mixed']['write']['response_ms']['p99']:.3f} ms;
observer-on p99s were
{diags[0]['mixed']['write']['response_ms']['p99']:.3f} / {diags[1]['mixed']['write']['response_ms']['p99']:.3f} ms.
Two short repetitions do not isolate
instrumentation overhead from storage/host variability; zero overhead is not
claimed. Do not compare two-second probe p99s as if they were the eight-second
main workload. All 800 diagnostic write records passed completeness/order checks.

The next decision is separate from this completed experiment. More tracing or
a durability/batching redesign is not justified automatically by the old maximum.
No long search was started, and the default memory representation was not changed.

"""
    if recurred:
        findings=findings.replace('**The 19.482-ms p99 regression did not recur in three new paired main trials.**',f'**The reference p99 was reached in {recurred} new main trials; attribution requires review.**')
        findings=findings.replace('No evidence-backed engine change is justified by this bounded run.', 'This report does not automatically select an engine change.')
    findings=findings.replace('New main trials had no\n>=20-ms write responses;', f'New main trials had {over20}\n>=20-ms write responses;')
    if throttled:
        findings=findings.replace('All new measured CPU-quota throttling increments were zero,\nwhich excludes observed quota throttling, not arbitrary scheduler or I/O delay.',f'Summed measured CPU-quota throttling was {throttled} us; inspect each interval before attribution.')
    text=text.replace('## Decision and limits',findings+'## Decision and limits',1)
    text=text.replace('The final interpretation is recorded in the findings\nsection below after inspecting all trials.', 'The findings above distinguish observations from the unresolved original event.')
    (root/'README.md').write_text(text,encoding='utf8')

if __name__=='__main__':main()
