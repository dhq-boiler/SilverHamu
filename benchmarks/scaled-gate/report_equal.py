"""Compare complete, uncoalesced production checkpoints and writer handoff envelopes."""
import argparse
from bisect import bisect_right
import gzip
import json
import math
from pathlib import Path
from statistics import median

from report_independent import analyze, overlap, records, save


def handoffs(log, ops, cap):
    attempts = records(log, "ATTEMPT")
    publications = records(log, "PUBLICATION")
    assert [c[0] for c in publications] == list(range(1, ops // cap))
    holds = sorted(records(log, "HOLD"), key=lambda h: h[2])
    assert len(holds) == len(attempts) + 2 * len(publications)
    for lane in range(4):
        lane_holds = [h for h in holds if h[0] == lane]
        lane_attempts = [a for a in attempts if a[0] % 4 == lane]
        assert len(lane_holds) == len(lane_attempts)
        for a,h in zip(lane_attempts,lane_holds):
            assert a[3] <= h[1] <= h[2] <= h[3] <= h[4] <= a[9]
    assert sum(h[0] == 4 for h in holds) == 2 * len(publications)
    assert all(s <= a <= b <= e for _,s,a,b,e in holds)
    starts = sorted(h[1] for h in holds)
    acquired = sorted(h[2] for h in holds)
    first, last = min(a[1] for a in attempts), max(a[9] for a in attempts)
    finished = max(last, publications[-1][-2])
    waiting_gaps = []
    switched_ns = 0
    drained_gap_ns = 0
    switches = 0
    release_overlap = 0
    active_gap_ns = 0
    for prev,nxt in zip(holds,holds[1:]):
        before,after = prev[3:5]
        observed = nxt[2]
        assert before <= observed
        if bisect_right(starts,before) == bisect_right(acquired,before):
            continue
        if prev[0] != nxt[0]:
            switches += 1
            switched_ns += observed-before
        drained_gap_ns += overlap(before,observed,first,finished)
        # Endpoint probes bracket unlock; the next probe follows lock acquisition.
        waiting_gaps.append(observed-before)
        release_overlap += observed - min(after,observed)
        active_gap_ns += overlap(before,observed,first,last)
    assert waiting_gaps and sum(waiting_gaps) >= release_overlap
    durable_ids = [a[10] for a in attempts if a[10]]
    base = min(durable_ids) - 1
    trigger_commits = {a[10]-base: a for a in attempts if a[10] and (a[10]-base)%cap==0}
    assert all(trigger_commits[c[0]*cap][9] <= c[3] for c in publications)
    sync_intervals=sorted((r[6],r[7]) for r in attempts if r[10])
    def coincident(intervals):
        j=0
        total=0
        for a,b in sync_intervals:
            while j<len(intervals) and intervals[j][1]<=a:
                j+=1
            k=j
            while k<len(intervals) and intervals[k][0]<b:
                total+=overlap(a,b,*intervals[k])
                k+=1
        return total
    during_prefix=coincident([(c[1],c[2]) for c in publications])
    during_scan=coincident([(c[3],c[4]) for c in publications])
    during_snapshot_sync=coincident([(c[4],c[5]) for c in publications])
    held_writer=sum(overlap(h[2],h[3],first,finished) for h in holds if h[0]!=4)
    held_cp=sum(overlap(h[2],h[3],first,finished) for h in holds if h[0]==4)
    sync=sum(a[7]-a[6] for a in attempts if a[10])
    remaining=finished-first-held_writer-held_cp-drained_gap_ns
    assert 0<=during_prefix+during_scan+during_snapshot_sync<=sync
    assert remaining>=0 and held_writer>=sync
    return dict(contended=len(waiting_gaps), owner_changes=switches,
                cross_owner_mean_us=switched_ns/max(1,switches)/1e3,
                drained_envelope_ms=drained_gap_ns/1e6,other_commit_hold_ms=(held_writer-sync)/1e6,
                maintenance_hold_ms=held_cp/1e6,other_time_ms=remaining/1e6,
                sync_during_prefix_ms=during_prefix/1e6,sync_during_scan_ms=during_scan/1e6,
                sync_during_snapshot_sync_ms=during_snapshot_sync/1e6,
                mean_envelope_us=sum(waiting_gaps)/len(waiting_gaps)/1e3,
                p99_envelope_us=sorted(waiting_gaps)[math.ceil(.99*len(waiting_gaps))-1]/1e3,
                total_envelope_ms=sum(waiting_gaps)/1e6,
                after_unlock_return_to_acquired_ms=release_overlap/1e6,
                active_envelope_ms=active_gap_ns/1e6,
                observable_request_ms=(last-first)/1e6, observable_drained_ms=(finished-first)/1e6,
                own_sync_ms=sum(a[7]-a[6] for a in attempts if a[10])/1e6,
                cutover_hold_ms=sum(c[10]-c[6] for c in publications)/1e6,
                cp_started_after_responses=sum(c[3]>last for c in publications))


def chart(summary,path):
    metrics=(("updates_s","Updates/s through last response"),("drained_s","Updates/s through final publication"),
             ("p99_ms","Update p99 (ms)"),("max_ms","Worst update (ms)"),
             ("KiB_per_update","Logical KiB/update, including drain"),("mean_envelope_us","Contended handoff envelope (µs)"))
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 920 485" role="img"><title>Equal checkpoint count and independent writers</title><rect width="920" height="485" fill="white"/><g font-family="sans-serif" fill="#203345">']
    def label(x,y,t,size):
        s.append(f'<text x="{x}" y="{y}" font-size="{size}">{t}</text>')
    label(16,30,"Four writers with 63 independent checkpoint publications",23)
    label(16,55,"One million rows, two CPUs, 4 GiB. Trial medians; worst update is the maximum across runs.",14)
    for i,(key,title) in enumerate(metrics):
        x,y=16+455*(i%2),90+130*(i//2)
        label(x,y,title,17)
        for j,(name,row,color) in enumerate(zip(("Standard","FIFO"),summary,("#506e87","#29755c"))):
            yy=y+32+32*j
            width=205*row[key]/max(r[key] for r in summary)
            label(x,yy,name,14)
            s.append(f'<rect x="{x+76}" y="{yy-13}" width="{width:.2f}" height="15" fill="{color}"/>')
            label(x+83+width,yy,f"{row[key]:,.2f}",14)
    path.write_text("\n".join(s+["</g></svg>",""]),encoding="utf8")


def accounting_chart(trials,path):
    parts=(("own_sync_ms","Commit WAL sync","#345d84"),("other_commit_hold_ms","Other commit hold","#87a9bd"),
           ("maintenance_hold_ms","Checkpoint lock","#a96738"),("drained_envelope_ms","Handoff envelope","#393f63"),
           ("other_time_ms","Remaining time","#549374"))
    means=[]
    for mode in ("off","on"):
        selected=[r for r in trials if r["mode"]==mode]
        values=[sum(r[k] for r in selected)/len(selected)/1000 for k,_,_ in parts]
        elapsed=sum(r["observable_drained_ms"] for r in selected)/len(selected)/1000
        assert abs(sum(values)-elapsed)<1e-6
        means.append(values)
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 900 350" role="img"><title>Elapsed time including drain</title><rect width="900" height="350" fill="white"/><g font-family="sans-serif" fill="#203345">']
    def text(x,y,value,size):
        s.append(f'<text x="{x}" y="{y}" font-size="{size}">{value}</text>')
    text(18,32,"Where time elapsed through the last response and publication",22)
    text(18,57,"Arithmetic means of three trials. All segments are disjoint wall-clock intervals.",14)
    maximum=max(map(sum,means))
    for j,(policy,vals) in enumerate(zip(("Standard","FIFO"),means)):
        x=106;y=90+34*j;text(18,y,policy,15)
        for value,(_,_,color) in zip(vals,parts):
            width=650*value/maximum
            s.append(f'<rect x="{x:.2f}" y="{y-15}" width="{width:.2f}" height="18" fill="{color}"/>')
            x+=width
        text(round(x+8),y,f"{sum(vals):.3f} s",14)
    text(300,169,"Standard (s)",15);text(485,169,"FIFO (s)",15);text(650,169,"Difference (s)",15)
    for i,(_,name,color) in enumerate(parts):
        y=199+29*i
        s.append(f'<rect x="19" y="{y-12}" width="12" height="12" fill="{color}"/>')
        text(40,y,name,15);text(300,y,f"{means[0][i]:.3f}",15);text(485,y,f"{means[1][i]:.3f}",15)
        text(650,y,f"{means[1][i]-means[0][i]:+.3f}",15)
    path.write_text("\n".join(s+["</g></svg>",""]),encoding="utf8")

def main():
    p=argparse.ArgumentParser()
    p.add_argument("raw",type=Path);p.add_argument("out",type=Path)
    args=p.parse_args()
    cfg=json.loads((args.raw/"environment.json").read_text())
    runs=json.loads((args.raw/"runs.json").read_text())
    assert cfg["equal_publications"] and cfg["fifo_comparison"]
    assert sorted((r["mode"],r["rep"]) for r in runs)==[(m,i) for m in ("off","on") for i in (1,2,3)]
    args.out.mkdir(exist_ok=True,parents=True)
    trials=[];phases=[];checkpoints=[]
    for r in runs:
        mode,rep=r["mode"],r["rep"]
        f=args.raw/f"{rep}-{mode}.log"
        log=f.read_text() if f.exists() else gzip.decompress(f.with_suffix(".log.gz").read_bytes()).decode()
        result,parts,cp=analyze(log,mode,rep,cfg["ops"],cfg["cap"])
        result.update(handoffs(log,cfg["ops"],cfg["cap"]))
        assert result["coalesced"]==result["unhandled"]==0
        trials.append(result);phases.extend(parts);checkpoints.extend(cp)
    save(args.out/"trials.csv",trials);save(args.out/"request-phases.csv",phases);save(args.out/"checkpoints.csv",checkpoints)
    metrics=("updates_s","drained_s","p99_ms","max_ms","KiB_per_update","write_MiB","published_after_last_response",
             "final_drain_ms","contended","owner_changes","mean_envelope_us","p99_envelope_us","total_envelope_ms",
             "after_unlock_return_to_acquired_ms","active_envelope_ms","cross_owner_mean_us","drained_envelope_ms","other_commit_hold_ms","maintenance_hold_ms","other_time_ms","observable_request_ms","observable_drained_ms","own_sync_ms","cutover_hold_ms")
    summary=[dict(mode=m,**{k:(max if k=="max_ms" else median)(r[k] for r in trials if r["mode"]==m) for k in metrics}) for m in ("off","on")]
    save(args.out/"summary.csv",summary);chart(summary,args.out/"comparison.svg");accounting_chart(trials,args.out/"accounting.svg")

if __name__=="__main__":
    main()
