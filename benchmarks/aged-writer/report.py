"""Render the lock comparison from verified per-run observations."""
import argparse
import csv
import math
from pathlib import Path
from statistics import median
from xml.etree.ElementTree import parse
from xml.sax.saxutils import escape

MODES=("standard","fifo","aged")
NAMES={"standard":"Current","fifo":"FIFO","aged":"Aged writer priority (16 ms)"}
def read(path):
    with path.open(newline="") as f:return list(csv.DictReader(f))

def values(rows,mode,key):
    return [float(r[key]) for r in sorted(rows,key=lambda x:int(x["rep"])) if r["mode"]==mode]

def graph(path,title,metrics,rows):
    height=88+len(metrics)*150
    parts=[f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 {height}" role="img">',
           f'<title>{escape(title)}</title><desc>Three points per lock, joined by trial number.</desc>',
           '<rect width="100%" height="100%" fill="white"/><g font-family="system-ui,sans-serif" fill="#203449">',
           f'<text x="20" y="32" font-size="22">{escape(title)}</text>',
           '<text x="20" y="56" font-size="13">Each line joins one trial. Labels give three-run medians, except for worst update (maximum).</text>']
    for index,(label,key,agg,log_scale) in enumerate(metrics):
        level=82+index*150
        transform=math.log1p if log_scale else lambda v:v
        high=max(transform(float(r[key])) for r in rows) or 1
        xs=[155,395,635]
        bottom=level+98
        parts.append(f'<text x="20" y="{level+5}" font-size="16">{escape(label)}</text>')
        for frac in (0,.5,1):
            cy=bottom-76*frac
            tick=math.expm1(high*frac) if log_scale else high*frac
            parts.append(f'<path d="M69 {cy}h670" stroke="#dae1e7"/><text x="62" y="{cy+4}" text-anchor="end" font-size="11">{tick:,.1f}</text>')
        for i,(color,dash) in enumerate([("#176987","none"),("#ad542a","6 3"),("#655695","2 3")]):
            ys=[bottom-76*transform(values(rows,m,key)[i])/high for m in MODES]
            points=" ".join(f"{x},{y}" for x,y in zip(xs,ys))
            parts.append(f'<polyline points="{points}" fill="none" stroke="{color}" stroke-width="2" stroke-dasharray="{dash}"/>')
            for x,y in zip(xs,ys):parts.append(f'<circle cx="{x}" cy="{y}" r="3" fill="{color}"/>')
        for m,x in zip(MODES,xs):
            value=agg(values(rows,m,key))
            parts.append(f'<text x="{x}" y="{bottom+20}" text-anchor="middle" font-size="14">{NAMES[m]} {value:,.2f}</text>')
    path.write_text("".join(parts)+"</g></svg>",encoding="utf8");parse(path)

def main():
    p=argparse.ArgumentParser();p.add_argument("directory",type=Path);args=p.parse_args();out=args.directory
    trials=read(out/"trials.csv");audit=read(out/"overtaking.csv")
    new=[r for r in audit if r["series"]=="new"];prior=[r for r in audit if r["series"]=="previous"]
    assert len(trials)==len(new)==9 and len(prior)==12
    perf=[("Sustained updates/s to last acknowledgement","updates_s",median,False),("Update p99 (ms)","p99_ms",median,False),("Worst update (ms)","max_ms",max,False)]
    graph(out/"comparison.svg","Writer handoff under independent checkpoints",perf,trials)
    graph(out/"overtaking.svg","Overtaking among competing updates",[
        ("Median passes in the slowest 1% (log1p scale)","top_median_passes",median,True),
        ("Passes per run starting at least 16 ms later (log1p scale)","late16",median,True),
        ("Maximum passes experienced by one update (log1p scale)","max_passes",max,True)],new)
    table=[];diff=[]
    for title,k,fn,_ in perf+[("Updates/s including checkpoint drain","drained_s",median,False),("WAL journal p99 (ms)","journal_p99_ms",median,False),
                             ("Final checkpoint path (ms)","publish_ms",median,False),("Application writes (MiB)","write_MiB",median,False)]:
        vals=[fn(values(trials,m,k)) for m in MODES]
        table.append("| "+title+" | "+" | ".join(f"{v:,.2f}" for v in vals)+f" | {(vals[2]/vals[0]-1)*100:+.1f}% | {(vals[2]/vals[1]-1)*100:+.1f}% |")
        if k in ("updates_s","p99_ms","max_ms"):
            sign=1 if k=="updates_s" else -1
            for ref in ("standard","fifo"):
                base=values(trials,ref,k);aged=values(trials,"aged",k)
                changes=[100*(b/a-1) for a,b in zip(base,aged)]
                diff.append(f"| {title} | {NAMES[ref]} | {sum(sign*c>0 for c in changes)}/3 | "+ " | ".join(f"{v:+.1f}%" for v in changes)+" |")
    totals=[]
    for m in MODES:
        group=[r for r in new if r["mode"]==m]
        n=lambda k:sum(int(r[k]) for r in group)
        totals.append(f"| {NAMES[m]} | {n('all_passed_requests'):,} | {n('total_passes'):,} | {n('late16'):,} | {max(int(r['max_passes']) for r in group):,} | {n('top_passed')}/492 |")
    detail=[]
    for row in sorted(new,key=lambda r:(int(r["rep"]),MODES.index(r["mode"]))):
        detail.append("| "+" | ".join([NAMES[row["mode"]],row["rep"],row["top_passed"],row["top_median_passes"],row["worst_passes"],row["worst_after16"],f'{float(row["worst_peer_sync_ms"]):,.2f}',f'{float(row["worst_longest_sync_ms"]):,.2f}'])+" |")
    worst=max(prior,key=lambda r:float(r["max_ms"]))
    assert (worst["worst_id"],worst["worst_passes"])==("4853","1012")
    higher=lambda k,fn,m:fn(values(trials,m,k))
    sustained=[higher("updates_s",median,m) for m in MODES]
    p99=[higher("p99_ms",median,m) for m in MODES]
    mx=[higher("max_ms",max,m) for m in MODES]
    old_counts=[sum(int(r["top_passed"]) for r in prior if r["mode"]==m) for m in MODES[:2]]

    paired=[]
    for k,sign in (("max_ms",-1),("p99_ms",-1),("updates_s",1)):
        base,candidate=[values(trials,m,k) for m in ("standard","aged")]
        paired.append(sum(sign*(y-x)>0 for x,y in zip(base,candidate)))
    conclusion=f"The aged writer priority policy improved maximum latency in {paired[0]}/3 pairs, p99 in {paired[1]}/3, and throughput in {paired[2]}/3 against the current lock. The strict FIFO comparison below shows the cost or benefit of permitting brief barging. A higher median rate alone cannot establish throughput retention in each run."
    report=f"""# Aged writer priority (16 ms)

The current lock, strict FIFO and aged writer priority policy sustained {sustained[0]:.2f}, {sustained[1]:.2f} and {sustained[2]:.2f} updates/s. Update p99 was {p99[0]:.2f}, {p99[1]:.2f} and {p99[2]:.2f} ms. The worst updates across three trials lasted {mx[0]:.2f}, {mx[1]:.2f} and {mx[2]:.2f} ms.

{conclusion}

![Performance by trial](comparison.svg)

| Measure | Current | FIFO | Aged writer priority (16 ms) | Change vs current | Change vs FIFO |
|---|---:|---:|---:|---:|---:|
{chr(10).join(table)}

Rates and p99 summarize trial-level medians. Maximum uses the worst request across all trials. Individual paired changes below use each trial's own maximum. Dumps contain every attempt, including conflicts and retries.

| Measure | Reference | Pairs improving | Pair 1 | Pair 2 | Pair 3 |
|---|---|---:|---:|---:|---:|
{chr(10).join(diff)}

## What happened in the earlier 6.80 s update

In the previous twelve runs the slowest request (#4853, current lock, writeback on, trial 2) waited 6,788.74 ms for the writer lock. Of 1,012 later-started attempts that acquired it first, 1,008 committed and 1,009 distinct request IDs were involved. Those passing commits used 6,649.90 ms of WAL sync. Including transactions already ahead, 1,011 WAL sync intervals overlapped its wait for 6,673.02 ms in total. The longest overlap was 20.50 ms. Of the slowest 164 updates per trial, {old_counts[0]} of 984 had at least one pass with the current lock and {old_counts[1]} of 984 with strict FIFO. Every old worst-request decomposition is independently checked against the previous published trace results. In another earlier run with FIFO and writeback disabled, a 1,371.94 ms update had zero passes and 1,341.21 ms of overlap with one WAL sync.

Across 49,152 old current-lock updates with writeback on, 4,026 (8.2%) accumulated at least 16 ms of writer wait. This motivated a single experimental threshold. The new policy measures age per queued acquisition, so a request accumulating wait across retries can differ from a single registration exceeding 16 ms.

## How the prototype admits writers

An arriving thread takes the free lock immediately if nobody is queued or the oldest registration is younger than 16 ms. At or beyond 16 ms the oldest registered waiter has priority at the next free lock. Queued threads keep registration order, and release wakes the head. There is no batching timer. The candidate uses the FIFO queue machinery and timestamps contended registrations. The baseline uses Rust's standard mutex. An owner may hold the lock through a slow sync and the eligible thread may wait for CPU time, so the threshold does not bound request latency.

![Overtaking by trial](overtaking.svg)

| Lock (49,152 requests each) | Requests passed at least once | Total passes | Passes starting ≥16 ms later | Largest count per request | Passed in slowest 1% |
|---|---:|---:|---:|---:|---:|
{chr(10).join(totals)}

| Lock | Trial | Slowest 164 with passes | Median passes among 164 | Worst request passes | Of those starting ≥16 ms later | All peer sync overlap ms | Longest overlap ms |
|---|---:|---:|---:|---:|---:|---:|---:|
{chr(10).join(detail)}

A pass requires another writer's lock-attempt timestamp strictly after the waiting attempt began and its acquire timestamp strictly before that attempt acquired the lock. Timestamp ties do not qualify. Counts track writer attempts and exclude the checkpoint thread. The timestamps bracket the lock call and can include scheduling before registration. They do not identify OS sleep or the exact instant the internal queue was entered. [Per-trial overtaking and sleep-independent sync intersections](overtaking.csv).

## Workload and verification

Nine fresh WSL2/ext4 Docker volumes, one million 256-byte values, four continuous writers, two CPUs, 4 GiB without swap, 16,384 durable updates per trial and exactly 63 independent publications. All arms use 8 MiB checkpoint writeback. The lock order is current/FIFO/16 ms, FIFO/16 ms/current, then 16 ms/current/FIFO. The workload checks the retained snapshot, latest contents, committed generations and reopened values. The candidate's tests exercise both threshold boundaries, queued handoff and panic poisoning on Linux and Windows. The large checkpoint test injects storage failure and exits before and after synchronization/publication. The old-reader and conflict fixture checks point and range conflicts. [Safety outcomes](safety.txt).

Build the candidate with `cargo build --release --features aged-writer`. The aged-writer Cargo feature includes fifo-writer. Using fifo-writer alone selects strict FIFO. Ordinary builds use the current mutex. [All new trials](trials.csv) · [All new publications](publications.csv) · [Trace quality](quality.csv) · [Previous trial references](prior-trials.csv) · [Image IDs](images.json). The gzip CSVs contain only numeric application attempt records.

Build benchmarks/aged-writer/Dockerfile as catdb-aged-writer and use the observer image described in the [production writeback campaign](../production-range-writeback-2026-11-08/README.md). Run campaign_aged.py RAW REPORT. Use audit.py REPORT --fresh RAW to export the new numeric input and replay the published historical markers. Add --old PREVIOUS_RAW_FULL to export the historical input from original application logs. Use audit.py REPORT to verify the self-contained published data. Generate the figures with report.py REPORT. Run test_analysis.py for timestamp-boundary checks. Inside the test image, mount ext4 at /build/artifacts and run the ignored range_writeback_recovers_after_failure_and_exit_around_final_sync test with CATDB_TEST_RANGE_WRITEBACK=1 and --features aged-writer.
"""
    (out/"README.md").write_text(report,encoding="utf8")
    print("\n".join(table+diff),flush=True)
if __name__=="__main__":main()
