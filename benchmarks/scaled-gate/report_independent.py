"""Reconcile attempts, checkpoint triggers and monotonic phase timestamps."""
import argparse
import csv
import gzip
import json
import math
import re
from collections import defaultdict
from pathlib import Path
from statistics import median

def records(log,tag):
    return [tuple(map(int,x.split(","))) for x in re.findall(tag+r",([0-9,]+)",log)]

def overlap(a,b,c,d):
    return max(0,min(b,d)-max(a,c))

def save(path,rows):
    with path.open("w",newline="") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)

def analyze(log,mode,rep,ops,cap):
    attempts=records(log,"ATTEMPT"); cps=records(log,"PUBLICATION")
    assert records(log,"INDEPENDENT_OK")==[(ops,)]
    grouped=defaultdict(list)
    for x in attempts:
        id,received,begin,commit,lock,acq,sync,flushed,size,end,seq=x
        assert received<=begin<=commit<=lock<=acq<=end
        if seq: assert acq<=sync<=flushed<=end and size>0
        else: assert (sync,flushed,size)==(0,0,0)
        grouped[id].append(x)
    assert sorted(grouped)==list(range(ops))
    assert all(len({x[1] for x in group})==1 and all(not x[-1] for x in group[:-1]) and group[-1][-1] for group in grouped.values())
    commits=sorted(x[-1] for x in attempts if x[-1]);assert commits==list(range(commits[0],commits[0]+ops))
    assert [c[0] for c in cps]==sorted(set(c[0] for c in cps)) and cps[-1][0]==(ops-1)//cap
    assert all(list(c[1:-1])==sorted(c[1:-1]) for c in cps)
    arrivals=[v[0][1] for v in grouped.values()]
    ends=[v[-1][-2] for v in grouped.values()]
    first=min(arrivals);last=max(ends);drain=max(last,cps[-1][-2])
    lag=sorted((v[-1][-2]-v[0][1],k) for k,v in grouped.items())
    durations=[]
    for label,idx in [('p99',math.ceil(.99*ops)-1),('worst',-1)]:
        elapsed,id=lag[idx];rows=grouped[id]
        wait=sum(x[5]-x[4] for x in rows)
        sync=sum(x[7]-x[6] for x in rows)
        prep=sum(x[3]-x[2] for x in rows)
        outside_commit=elapsed-sum(x[9]-x[3] for x in rows)
        cp_lock=sum(overlap(x[4],x[5],c[6],c[10]) for x in rows for c in cps)
        peer_intervals=[dt for x in rows for y in attempts if y[0]!=id and y[10]
                        if (dt:=overlap(x[4],x[5],y[6],y[7]))]
        other_sync=sum(peer_intervals)
        assert cp_lock+other_sync<=wait
        durations.append(dict(mode=mode,rep=rep,label=label,id=id,lane=id%4,retries=len(rows)-1,total_ms=elapsed/1e6,
              prepare_ms=prep/1e6,before_attempt_and_retry_gaps_ms=(outside_commit-prep)/1e6,
              writer_wait_ms=wait/1e6,writer_wait_cp_hold_ms=cp_lock/1e6,
              writer_wait_peer_sync_ms=other_sync/1e6,peer_sync_intervals=len(peer_intervals),
              longest_peer_sync_overlap_ms=max(peer_intervals,default=0)/1e6,writer_wait_unattributed_ms=(wait-cp_lock-other_sync)/1e6,
              own_sync_ms=sync/1e6,other_commit_ms=(elapsed-outside_commit-wait-sync)/1e6,
              cp_overlap_ms=sum(overlap(rows[0][1],rows[-1][9],c[3],c[11]) for c in cps)/1e6))
    period=[]
    for c in cps:
        due,start,ready,cp,written,synced,locked,copied,tail_synced,replaced,released,end,bytes_written=c
        period.append(dict(mode=mode,rep=rep,due=due,prefix_ms=(ready-start)/1e6,checkpoint_ms=(end-cp)/1e6,
                   snapshot_sync_ms=(synced-written)/1e6,writer_hold_ms=(released-locked)/1e6,
                   tail_copy_ms=(copied-locked)/1e6,tail_sync_ms=(tail_synced-copied)/1e6,
                   post_tail_to_replaced_ms=(replaced-tail_synced)/1e6,bytes=bytes_written,
                   arrivals_during_checkpoint=sum(cp<=a<end for a in arrivals),
                   completions_during_checkpoint=sum(cp<=e<end for e in ends),
                   arrivals_during_writer_hold=sum(locked<=a<released for a in arrivals),
                   finished_after_last_response=int(end>last)))
    appended=sum(x[8] for x in attempts); replacements=sum(c[-1] for c in cps)
    trial=dict(mode=mode,rep=rep,updates=ops,checkpoints=len(cps),triggers=(ops-1)//cap,
            coalesced=(ops-1)//cap-len(cps),unhandled=0,
            published_after_last_response=sum(x["finished_after_last_response"] for x in period),
            final_drain_ms=(drain-last)/1e6,arrivals_during_checkpoint=sum(x["arrivals_during_checkpoint"] for x in period),
            arrivals_during_writer_hold=sum(x["arrivals_during_writer_hold"] for x in period),
            completions_during_checkpoint=sum(x["completions_during_checkpoint"] for x in period),
            p99_ms=durations[0]["total_ms"],max_ms=durations[1]["total_ms"],updates_s=ops*1e9/(last-first),
            drained_s=ops*1e9/(drain-first),append_B=appended,checkpoint_B=replacements,
            write_MiB=(appended+replacements)/2**20,KiB_per_update=(appended+replacements)/1024/ops,
            MiB_per_publication=replacements/len(cps)/2**20)
    assert trial["arrivals_during_checkpoint"]>0 and trial["completions_during_checkpoint"]>0
    return trial,durations,period

def main():
    p=argparse.ArgumentParser();p.add_argument("raw",type=Path);p.add_argument("out",type=Path);a=p.parse_args()
    cfg=json.loads((a.raw/"environment.json").read_text());runs=json.loads((a.raw/"runs.json").read_text())
    assert sorted((r['mode'],r['rep']) for r in runs)==[(m,i) for m in ('off','on') for i in (1,2,3)]
    trials=[];phases=[];events=[]
    for r in runs:
        path=a.raw/f"{r['rep']}-{r['mode']}.log"
        data=path.read_text() if path.exists() else gzip.decompress(path.with_suffix(".log.gz").read_bytes()).decode()
        row,parts,windows=analyze(data,r['mode'],r['rep'],cfg['ops'],cfg['cap'])
        trials.append(row);phases.extend(parts);events.extend(windows)
    a.out.mkdir(exist_ok=True,parents=True)
    save(a.out/"trials.csv",trials);save(a.out/"request-phases.csv",phases);save(a.out/"checkpoints.csv",events)
    summary=[]
    for m in ('off','on'):
        rows=[r for r in trials if r['mode']==m]
        summary.append(dict(mode=m,**{k:(max if k=='max_ms' else median)(r[k] for r in rows) for k in
         ('p99_ms','max_ms','updates_s','drained_s','KiB_per_update','write_MiB','checkpoints','MiB_per_publication')}))
    save(a.out/"summary.csv",summary)
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 920 380" role="img"><title>Independent production checkpoints</title><rect width="920" height="380" fill="white"/><g font-family="sans-serif" fill="#213248">']
    def label(x,y,s,size):svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(16,30,"Independent checkpoints with four continuously admitted writers",22)
    label(16,58,"One million rows, two CPUs, 4 GiB. Trial medians except maximum across all three runs.",14)
    for i,(key,title) in enumerate([('p99_ms','Update p99 (ms)'),('max_ms','Worst update (ms)'),('updates_s','Updates / second'),('KiB_per_update','Logical KiB / update, including drain')]):
        x,y=16+i%2*455,93+i//2*140;label(x,y,title,17)
        for j,(r,name,color) in enumerate(zip(summary,('Off','Prefix on'),('#536982','#297465'))):
            yy=y+32+j*33;width=202*r[key]/max(row[key] for row in summary)
            label(x,yy,name,14);svg.append(f'<rect x="{x+75}" y="{yy-12}" width="{width:.2f}" height="14" fill="{color}"/>');label(x+81+width,yy,f"{r[key]:,.2f}",14)
    (a.out/"comparison.svg").write_text("\n".join(svg+["</g></svg>",""]))
    worst=sorted((r for r in phases if r["label"]=="worst"),key=lambda v:(v["rep"],v["mode"]))
    bars=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 950 380" role="img"><title>Wall time of the slowest update in each trial</title><rect width="950" height="380" fill="white"/><g font-family="sans-serif" fill="#213248">']
    def caption(x,y,s,size=14):
        bars.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    caption(16,30,"Where the slowest update in each trial spent its time",22)
    caption(16,55,"Absolute wall time; each millisecond is assigned to one segment.")
    colors=["#355d93","#ce7d32","#92a5ba","#22715c","#c1cbc3"]
    for i,name in enumerate(("Wait: peer sync","Wait: checkpoint lock","Other lock wait","Own sync","Remaining request")):
        x=16+183*i
        bars.append(f'<rect x="{x}" y="75" width="12" height="12" fill="{colors[i]}"/>')
        caption(x+17,86,name,13)
    high=max(r["total_ms"] for r in worst)
    for i,r in enumerate(worst):
        y=120+39*i
        caption(16,y,f"Prefix {r['mode']} / {r['rep']}")
        parts=[r[k] for k in ("writer_wait_peer_sync_ms","writer_wait_cp_hold_ms","writer_wait_unattributed_ms","own_sync_ms")]
        parts.append(r["total_ms"]-sum(parts))
        assert min(parts)>=-1e-6
        x=113.0
        for part,color in zip(parts,colors):
            w=700*max(0,part)/high
            bars.append(f'<rect x="{x:.2f}" y="{y-14}" width="{w:.2f}" height="18" fill="{color}"/>')
            x+=w
        caption(x+8,y,f"{r['total_ms']:,.1f} ms")
    (a.out/"attribution.svg").write_text("\n".join(bars+["</g></svg>",""]))
if __name__=="__main__":main()
