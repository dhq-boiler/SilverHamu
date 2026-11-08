"""Generate the decision tables and SVGs from paired trials and nonoverlapping wall-time bins."""
import argparse
import csv
import math
from pathlib import Path
from statistics import median
from xml.etree.ElementTree import parse
from xml.sax.saxutils import escape

MODES=("standard","fifo","aged")
CASES=("single10","four20","continuous")
NAMES=dict(standard="Current",fifo="FIFO",aged="Aged writer priority (16 ms)",single10="1 writer, 10/s",four20="4 writers, 20/s",continuous="4 writers, continuous")
COLORS=("#426786","#8e5c31","#006c64")

def load(path):
    with path.open(newline="") as f:return list(csv.DictReader(f))

def series(rows,case,mode,key):
    return [float(r[key]) for r in sorted(rows,key=lambda r:int(r["rep"])) if r["case"]==case and r["mode"]==mode]

def differences(rows,case,key,ref="standard"):
    return [b-a for a,b in zip(series(rows,case,ref,key),series(rows,case,"aged",key))]

def svg(path,title,body,height,width=1050):
    path.write_text(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img"><title>{escape(title)}</title><rect width="100%" height="100%" fill="#fff"/><g font-family="system-ui,sans-serif" fill="#19334a">'+''.join(body)+'</g></svg>',encoding="utf8")
    parse(path)

def label(x,y,t,size=13,anchor="start"):
    return f'<text x="{x}" y="{y}" text-anchor="{anchor}" font-size="{size}">{escape(t)}</text>'

def performance(path,rows):
    title="Update performance across arrival rates"
    s=[label(20,32,title,24),label(20,57,"Aged = Aged writer priority (16 ms). Points show trials. Bars show medians, or the worst trial for maximum latency.",14)]
    metrics=(("Dispatch-to-completion p99 (ms)","sent_p99_ms",median),("Planned-to-completion p99 (ms)","scheduled_p99_ms",median),
             ("Maximum planned-to-completion (ms)","max_scheduled_ms",max),("Acknowledged updates/s","updates_s",median))
    for i,(name,key,agg) in enumerate(metrics):
        y=88+i*144;s.append(label(20,y,name,17))
        for col,case in enumerate(CASES):
            x=20+col*345;s.append(label(x,y+22,NAMES[case],13))
            hi=max(float(r[key]) for r in rows if r["case"]==case) or 1
            for j,m in enumerate(MODES):
                yy=y+43+j*25; v=series(rows,case,m,key); a=agg(v);start=x+63
                s.append(label(x,yy,"Aged" if m=="aged" else NAMES[m]))
                s.append(f'<path d="M{start} {yy-4}h190" stroke="#dce3e6"/>')
                s.append(f'<rect x="{start}" y="{yy-8}" width="{190*a/hi:.2f}" height="8" fill="{COLORS[j]}" opacity=".60"/>')
                for k,value in enumerate(v):
                    s.append(f'<circle cx="{start+190*value/hi:.2f}" cy="{yy-4+2*(k-1)}" r="2.8" fill="{COLORS[j]}"><title>Trial {k+1}: {value:,.3f}</title></circle>')
                s.append(label(start+199,yy,f"{a:,.2f}"))
    s.append(label(20,656,"Scales are local to each panel. Continuous demand starts its latency clock at dispatch.",12))
    svg(path,title,s,678)

def attribution(path,old):
    title="Where the previous elapsed-time difference accumulated"
    s=[label(20,32,title,23),label(20,57,"Aged writer priority (16 ms) minus current lock, seconds per 16,384 updates. Positive values mean more elapsed time.",14)]
    for i,(name,key) in enumerate((("Elapsed through last response","response_s"),("WAL sync","wal_s"),("Contended handoff envelope","handoff_s"))):
        x=22+i*340;axis=x+150;s.append(label(x,93,name,15))
        v=[float(old["aged",r][key])-float(old["standard",r][key]) for r in range(1,4)]
        unit=max(map(abs,v)) or 1
        s.append(f'<path d="M{axis} 107v117" stroke="#8396a2"/>')
        for r,value in enumerate(v):
            y=128+37*r;w=100*abs(value)/unit;left=axis-w if value<0 else axis
            s.append(label(x,y,f"Pair {r+1}"))
            s.append(f'<rect x="{left}" y="{y-12}" width="{w}" height="14" fill="{COLORS[0] if value<0 else COLORS[1]}"/>')
            s.append(label(x+257,y,f"{value:+.3f}"))
    s.append(label(20,251,"Each panel uses its own scale. The handoff envelope includes release and rescheduling around admission.",13))
    svg(path,title,s,272)

def main():
    p=argparse.ArgumentParser();p.add_argument("out",type=Path);args=p.parse_args();out=args.out
    rows=load(out/"trials.csv");diag=load(out/"diagnostics.csv")
    for r in rows:
        extra=next(x for x in diag if all(x[k]==r[k] for k in ("mode","case","rep")));r.update(extra)
    old={(r["mode"],int(r["rep"])):r for r in load(out/"previous.csv")}
    oswait={(r["mode"],int(r["rep"])):r for r in load(out/"previous-kernel.csv")}
    performance(out/"load.svg",rows);attribution(out/"previous.svg",old)
    hist=[]
    for rep in (1,2,3):
        a,b=old["standard",rep],old["aged",rep]
        delta=lambda k:float(b[k])-float(a[k])
        journal=float(oswait["aged",rep]["journal_sleep_s"])-float(oswait["standard",rep]["journal_sleep_s"])
        hist.append("| "+str(rep)+" | "+" | ".join(f"{delta(k):+.3f}" for k in ("response_s","wal_s","handoff_s","checkpoint_hold_s","other_writer_s","remaining_s"))+f" | {journal:+.3f} |")
    hot={(r["mode"],int(r["rep"])):r for r in rows if r["case"]=="continuous"}
    fresh=[]
    for rep in (1,2,3):
        a,b=hot["standard",rep],hot["aged",rep]
        parts=[float(b[k])-float(a[k]) for k in ("response_s","wal_s","handoff_s","checkpoint_hold_s","other_writer_s","remaining_s")]
        fresh.append("| "+str(rep)+" | "+" | ".join(f"{d:+.3f}" for d in parts)+" |")
    table=[];paired=[];internal=[];light_detail=[]
    for case in CASES:
        for mode in MODES:
            values=lambda key,fn=median:fn(series(rows,case,mode,key))
            cells=[NAMES[case],NAMES[mode]]+[f"{values(k,fn):,.2f}" for k,fn in (("updates_s",median),("scheduled_p99_ms",median),("sent_p99_ms",median),
                     ("max_scheduled_ms",max),("max_sent_ms",max),("send_lag_p99_ms",median),("wal_p99_ms",median))]
            table.append("| "+" | ".join(cells)+" |")
            internal.append("| "+" | ".join([NAMES[case],NAMES[mode]]+[f"{values(k):,.3f}" for k in ("lock_median_us","lock_p99_us","handoff_s","wal_s","checkpoint_hold_s","drained_rate")])+" |")
            if case!="continuous":
                subset=[r for r in rows if r["case"]==case and r["mode"]==mode]
                light_detail.append("| "+" | ".join([NAMES[case],NAMES[mode],str(sum(int(r["isolated_n"]) for r in subset)),
                        f"{values('isolated_median_us'):.3f}",f"{values('isolated_p99_us'):.3f}",
                        str(sum(int(r["backlogged"]) for r in subset)),f"{values('max_backlog_ms',max):,.2f}"])+" |")
        for ref in ("standard","fifo"):
            for key,title in (("scheduled_p99_ms","Planned p99 (ms)"),("max_scheduled_ms","Worst planned latency (ms)"),("updates_s","Updates/s")):
                if case!="continuous" and key=="updates_s":continue
                changes=differences(rows,case,key,ref)
                sign=1 if key=="updates_s" else -1
                paired.append("| "+" | ".join([NAMES[case],NAMES[ref],title,str(sum(sign*c>0 for c in changes))+"/3"]+[f"{v:+,.2f}" for v in changes])+" |")
    better=[sum(sign*d>0 for d in differences(rows,"continuous",key)) for key,sign in (("scheduled_p99_ms",-1),("max_scheduled_ms",-1),("updates_s",1))]
    simultaneous=sum(all(sign*differences(rows,"continuous",key)[i]>=0 for key,sign in (("scheduled_p99_ms",-1),("max_scheduled_ms",-1),("updates_s",1))) for i in range(3))
    light=[sum(d>0 for d in differences(rows,case,"scheduled_p99_ms")) for case in CASES[:2]]
    conclusion=f"Against the current lock, continuous-load p99 improved in {better[0]}/3 pairs, worst latency in {better[1]}/3, and throughput in {better[2]}/3. All three held or improved together in {simultaneous}/3 pairs. Planned p99 was higher with the candidate in {light[0]}/3 single-writer light pairs and {light[1]}/3 four-writer light pairs."
    a,b=old["standard",1],old["aged",1]
    elapsed=float(b["response_s"])-float(a["response_s"]);wal=float(b["wal_s"])-float(a["wal_s"]);gap=float(b["handoff_s"])-float(a["handoff_s"])
    journal=float(oswait["aged",1]["journal_sleep_s"])-float(oswait["standard",1]["journal_sleep_s"])
    during=float(oswait["aged",1]["during_commit_s"])-float(oswait["standard",1]["during_commit_s"])
    report=f"""# Does the aged writer priority policy hold up across arrival rates?

{conclusion}

The current lock remains the default. The 16 ms threshold is useful when reducing long writer waits takes priority. In continuous pair 3 the worst request fell from {float(hot["standard",3]["max_scheduled_ms"])/1000:.2f} to {float(hot["aged",3]["max_scheduled_ms"])/1000:.2f} s, while acknowledged throughput fell from {float(hot["standard",3]["updates_s"]):.1f} to {float(hot["aged",3]["updates_s"]):.1f} updates/s. Strict FIFO delivered higher continuous throughput than the aged writer priority policy in all three runs. This study covers one ext4 environment with 8 MiB writeback enabled.

![Measured updates at three arrival rates](load.svg)

## Revisiting the 33.9% throughput drop

In the first pair of the earlier campaign, total time to the last response increased by **{elapsed:.3f} s**. The application WAL-sync interval accounted for **{wal:.3f} s ({100*wal/elapsed:.1f}%)** of the increase and the contended handoff envelope **{gap:.3f} s**. Inside the WAL interval, the change in guest journal-commit sleep was **{journal:+.3f} s**. Of that journal sleep difference, {during:+.3f} s occurred between the JBD2 commit start and end events. The last column is a subset of WAL time. The first five component columns sum to elapsed time, subject to rounding.

![Elapsed time, WAL and handoff by pair](previous.svg)

| Earlier pair | Elapsed s | WAL sync s | Handoff s | Checkpoint lock hold s | Other writer hold s | Remaining s | Journal sleep within WAL s |
|---:|---:|---:|---:|---:|---:|---:|---:|
{chr(10).join(hist)}

All intervals use the first request through the last acknowledgement. WAL and checkpoint hold times are serialized by the writer lock. The handoff envelope runs from just before unlock until the next observed acquisition when application markers show a contender. It includes scheduling and marker overhead. Idle time and gaps without such a contender go in the remainder. Guest sleep in jbd2_log_wait_commit is correlated with fsync by thread and monotonic timestamps. A lock policy can shift when WAL syncs coincide with other storage activity. The interval totals cannot separate that indirect effect from fluctuations elsewhere in the guest or host. In that first pair, {a['post_response_pubs']} current-lock publications and {b['post_response_pubs']} candidate publications finished after the last response. [Application accounting](previous.csv) · [Kernel evidence](previous-kernel.csv).

## Fresh measurements

Each lock ran three times per load on a fresh million-row ext4 Docker volume with 256-byte values, 2 CPUs, 4 GiB without swap, an old snapshot and 8 MiB checkpoint writeback. Each light trial ran 1,024 durable updates with three checkpoint publications. For one writer the global rate was 10/s, and for four writers it was 20/s. The continuous four-writer case ran 16,384 updates with 63 publications. Both load order and lock order rotated across repetitions. All runs used the same tracing configuration.

| Demand | Lock | Updates/s | Planned p99 ms | Dispatch p99 ms | Worst planned ms | Worst dispatch ms | Dispatch lag p99 ms | Own WAL p99 ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|
{chr(10).join(table)}

One common origin establishes the predetermined arrival schedule. Request IDs are assigned to workers round-robin. Each worker dispatches its next request once due and free, preserving delayed slots and retries. Dispatch is local entry to preparing the update. Planned latency includes generator or backlog delay. In the closed-loop continuous case a new request starts at dispatch. Light-load updates/s is governed by demand. All p99 columns are medians of three trial percentiles. Each worst column is the maximum over all trials.

| Demand | Lock | Lock acquisition median µs | Lock acquisition p99 µs | Contended handoff s | WAL sync s | Checkpoint hold s | Updates/s through drain |
|---|---|---:|---:|---:|---:|---:|---:|
{chr(10).join(internal)}

Lock acquisition brackets each writer's lock call, including OS scheduling. The handoff and hold columns accumulate wall time per trial. Locks and storage remain unchanged from the [original candidate](../aged-writer-2026-11-08/README.md).

The light-load check below selects calls where the previous unlock-return marker precedes this attempt, nobody was already waiting and no other attempt began through acquisition. This is observed wall time on an uncontended path. Lock time includes the timestamp probes. Backlogged requests had a previous request on the same lane still active at the scheduled arrival.

| Light demand | Lock | Isolated acquisitions, all runs | Median µs | P99 µs | Backlogged requests, all runs | Longest lane backlog ms |
|---|---|---:|---:|---:|---:|---:|
{chr(10).join(light_detail)}

For the fresh continuous runs, the elapsed-time components below use the same clock boundaries as the earlier comparison.

| Fresh pair, Aged writer priority (16 ms) minus current | Elapsed s | WAL sync s | Handoff s | Checkpoint hold s | Other writer hold s | Remaining s |
|---|---:|---:|---:|---:|---:|---:|
{chr(10).join(fresh)}

The [p99 and worst request in every trial](tail.csv) separates each selected request into dispatch lag, lock wait, its own WAL sync and other service time. Lane-busy time is included in dispatch lag.

## Paired changes

Positive means a larger value with the aged writer priority policy. Pairing uses the same repetition number. See the trial data for actual arrival lag and every maximum.

| Demand | Reference | Metric | Pairs improving | Pair 1 difference | Pair 2 difference | Pair 3 difference |
|---|---|---|---:|---:|---:|---:|
{chr(10).join(paired)}

## Integrity and reproduction

All 27 runs verified the held snapshot, committed version order, latest contents and reopened data. Successful updates total 165,888 over 621 publications. All planned IDs completed. Guest buffer loss and probe misses invalidate a capture. [Trials](trials.csv) · [Timing diagnostics](diagnostics.csv) · [Capture checks](quality.csv) · [Image IDs](images.json). The compressed request tables and numeric markers support percentile and accounting replay.

Build benchmarks/aged-writer/Dockerfile as catdb-aged-load, with catdb-fifo-products as the observer. Run `python benchmarks/aged-load/campaign.py RAW REPORT` for the new measurements. Run audit_load.py REPORT to verify exported markers and request records, then report.py REPORT to regenerate the figures. To export the original accounting markers and kernel rows, use audit_load.py with --previous-raw and kernel_previous.py with the original raw trace directory.
"""
    (out/"README.md").write_text(report,encoding="utf8");print(conclusion)
if __name__=="__main__":main()
