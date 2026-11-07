"""Compare the production API runs with each other and with the published prototype."""
import argparse
import csv
from pathlib import Path
from statistics import median
import sys

bench = Path(__file__).resolve().parents[1]
import os
bench = Path(os.environ.get("CATDB_BENCH_ROOT",bench))
sys.path.insert(0,str(bench/"range-writeback"))
from report_range import figure, load, values, verify

def main():
    p=argparse.ArgumentParser();p.add_argument("out",type=Path);args=p.parse_args()
    out=args.out
    trials=load(out/"trials.csv")
    publications=load(out/"publications.csv")
    verify(trials,publications,63,3)
    verify(load(out/"pilot-trials.csv"),load(out/"pilot-publications.csv"),2,1)
    old=load(bench.parent/"docs/benchmarks/range-writeback-2026-11-08/trials.csv")
    perf=[("Update throughput","updates_s","updates/s"),("Update p99","p99_ms","ms"),("Worst update per trial","max_ms","ms")]
    io=[("Rolling 10 ms peak, median across publications","peak10_MiB","MiB"),
        ("WAL-targeted journal commit p99","journal_p99_ms","ms"),
        ("Final checkpoint path, median across publications","publish_ms","ms")]
    figure(out/"performance.svg","Update performance with ext4 checkpoint writeback",perf,trials,label_aggregates={"max_ms":max})
    figure(out/"writeback.svg","ext4 submissions, WAL journal commits and publication",io,trials)
    table=[];pairs=[]
    for mode in ("standard","fifo"):
        for title,key,unit in perf+io+[("Submitted bytes in 10 ms bins of at least 32 MiB","bulk_pct","%"),("Throughput including drain","drained_s","updates/s"),
                                     ("Application writes including WAL","write_MiB","MiB")]:
            fn=max if key=="max_ms" else median
            a,b=[values(trials,mode,v,key) for v in ("baseline","range8")]
            x,y=[fn(values(old,mode,v,key)) for v in ("baseline","range8")]
            off,on=fn(a),fn(b)
            sign=1 if key in ("updates_s","drained_s") else -1
            improved=sum(sign*(new-base)>0 for base,new in zip(a,b))
            paired_change=median(100*(new/base-1) for base,new in zip(a,b) if base) if any(a) else 0
            table.append(f"| {mode} | {title} ({unit}) | {off:,.2f} | {on:,.2f} | {(on/off-1)*100:+.1f}% | {(y/x-1)*100:+.1f}% |")
            if key in ("updates_s","p99_ms","max_ms","journal_p99_ms"):
                pairs.append(f"| {mode} | {title} | {improved}/3 | {paired_change:+.1f}% | "+" | ".join(f"{base:,.1f} → {new:,.1f}" for base,new in zip(a,b))+" |")
    differences=[abs(b/a-1)*100 for mode in ("standard","fifo") for key in ("logical_B","submitted_B")
                 for a,b in zip(values(trials,mode,"baseline",key),values(trials,mode,"range8",key))]
    worst_peak=[max(int(p["peak10_B"]) for p in publications if p["variant"]==v)/2**20 for v in ("baseline","range8")]
    tail=[]
    samples=load(out/"tail-requests.csv")
    for mode in ("standard","fifo"):
        for variant in ("baseline","range8"):
            r=max((r for r in samples if (r["mode"],r["variant"],r["label"])==(mode,variant,"worst")),key=lambda r:float(r["total_ms"]))
            cols=[mode,variant,r["rep"]]+[f"{float(r[k]):,.2f}" for k in ("total_ms","writer_wait_ms","writer_wait_cp_hold_ms","writer_wait_peer_sync_ms","own_sync_ms")]
            tail.append("| "+" | ".join(cols)+" |")
    quality=load(out/"trace-quality.csv")
    assert len(quality)==12 and all(int(r["probe_misses"])==0 and int(r["ring_loss"])==0 and r["cleaned"]=="True" for r in quality)
    missing=""
    if (out/"excluded.csv").exists():
        discarded=load(out/"excluded.csv")
        lines=[f"| {r['mode']} | {r['variant']} | {r['rep']} | {r['kernel_loss']} | {float(r['updates_s']):.2f} | {float(r['update_p99_ms']):.2f} | {float(r['max_ms']):.2f} |" for r in discarded]
        missing=("## Incomplete kernel captures\n\nThe following trials completed the workload and reopened successfully. Kernel ring loss prevented a WAL journal p99. Their application results are preserved separately. Only the affected arm was repeated, with the same observer configuration.\n\n| Lock | Setting | Trial | Lost events | Updates/s | Update p99 ms | Maximum ms |\n|---|---|---:|---:|---:|---:|---:|\n"+"\n".join(lines)+"\n\n[Excluded captures](excluded.csv).")
        if len(discarded)==1:
            e=discarded[0];mode=e["mode"];i=int(e["rep"])-1
            a,b=[values(trials,mode,v,"updates_s") for v in ("baseline","range8")]
            retained=sum(y>x for x,y in zip(a,b))
            (a if e["variant"]=="baseline" else b)[i]=float(e["updates_s"])
            alternate=sum(y>x for x,y in zip(a,b))
            missing+=f" Using the incomplete attempt in an application-only throughput sensitivity check gives {alternate}/3 faster pairs for {mode}, compared with {retained}/3 in the retained campaign."
    def stats(m,k,fn=median):
        return [fn(values(trials,m,v,k)) for v in ("baseline","range8")]
    x,y=stats("standard","p99_ms"); a,b=stats("fifo","p99_ms"); f,g=stats("fifo","journal_p99_ms")
    doc=f"""# Production ext4 checkpoint range writeback

With the production configuration API, update p99 changed from **{x:.2f} to {y:.2f} ms** with the current lock and **{a:.2f} to {b:.2f} ms** with FIFO. FIFO WAL-targeted journal p99 changed from **{f:.2f} to {g:.2f} ms**. All twelve measurements used fresh volumes. Disabling and enabling writeback was the only engine setting changed within each lock pair.

![Per-pair performance](performance.svg)

| Lock | Measure | Disabled | Enabled | Production change | Prior prototype change |
|---|---|---:|---:|---:|---:|
{chr(10).join(table)}

Throughput and p99 are medians of three trials per arm. Maximum update is the worst across all three. Each figure line shows one trial pair. Its axis labels use the aggregates in the table. The previous column uses the separate [test-only prototype campaign](../range-writeback-2026-11-08/README.md).

![Measured guest writeback and journal timing](writeback.svg)

| Lock | Measure | Pairs improving | Median paired change | Pair 1 off → on | Pair 2 off → on | Pair 3 off → on |
|---|---|---:|---:|---:|---:|
{chr(10).join(pairs)}

## Slowest request in each arm

The two overlap columns are subsets of writer lock wait. Each row describes a single request, including retries.

| Lock | Setting | Trial | Total ms | Writer lock wait ms | Wait overlapping checkpoint lock hold ms | Wait overlapping other WAL syncs ms | Own WAL sync ms |
|---|---|---:|---:|---:|---:|---:|---:|
{chr(10).join(tail)}

## Conditions and measurement

One million 256-byte values, four writers, 2 CPUs, 4 GiB without swap, 16,384 durable updates and 63 independent checkpoint publications per trial on a WSL2 ext4 volume. The prefix is enabled in both arms. Each pair ran the same release-mode Rust test executable and observer with the order reversed in the second round. The workload called the public Database setter before the initial seed checkpoint and ran the ordinary production checkpoint code. Application latency includes retries. The journal interval is the matching JBD2 start to end for a WAL wait. Whole-request latency also covers lock waits and other writers. Publication time starts at handoff to the final checkpoint path and ends when the call returns. Worst absolute paired change in logical or guest-submitted checkpoint bytes was {max(differences):.3f}% because checkpoint timing can alter suffix length. Maximum observed checkpoint bytes in any rolling 10 ms window across all publications were {worst_peak[0]:.2f} MiB disabled and {worst_peak[1]:.2f} MiB enabled.

The observer matches data submissions by device, inode and checkpoint lifetime, checks for lost events, and verifies the application write total for every publication. Each trial verifies the held snapshot, final records, commit sequence and reopened data. [All trials](trials.csv) · [756 publications](publications.csv) · [Pilot trials](pilot-trials.csv) · [Trace quality](trace-quality.csv) · [Journal cohorts](journal-groups.csv) · [P99 and worst requests](tail-requests.csv) · [Image IDs](images.json). The compressed update and epoch tables are per trial.

{missing}

## Reproduction

Build benchmarks/scaled-gate/Dockerfile.fifo as catdb-prod-range8-final. Prepare catdb-fifo-products as described in the [previous benchmark](../ext4-checkpoint-bytes-2026-11-08/README.md). Run `python benchmarks/production-range-writeback/run.py FRESH_RAW_DIR FRESH_REPORT_DIR`. The container needs access to the guest trace events and a disposable ext4 volume. The service and C ABI checks exercise a separately built regular release library and executable.
"""
    (out/"README.md").write_text(doc,encoding="utf8")
    print(doc.splitlines()[2])

if __name__=="__main__":main()
