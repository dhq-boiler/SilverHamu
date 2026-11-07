"""Generate the four-writer report from run_four.py logs (standard library)."""
import argparse
import csv
import json
import math
from pathlib import Path
from statistics import median


def summarize(raw, run):
    assert run["status"] == "passed" and not run["state"]["OOMKilled"]
    _, mode, rows, ops, elapsed, rss, verified = run["summary"].split(",")
    assert mode == run["mode"] and verified == "verified"
    lines = (raw / (run["label"] + ".log")).read_text().splitlines()
    periods = [tuple(map(int, line.split(",")[2:])) for line in lines if line.startswith("FOUR_CHECKPOINT,")]
    samples = [tuple(map(int, line.split(",")[2:])) for line in lines if line.startswith("SCALED_REQUEST,")]
    assert int(rows)==1000000 and len(samples)==int(ops)==1024 and len(periods)==3
    assert sorted(s[0] for s in samples)==list(range(len(samples)))
    assert len(set(s[2] for s in samples))==len(samples)
    assert all(s[1]==s[0]%4 and s[4]>=s[3] for s in samples)
    lat=sorted((s[4]-s[3])/1e6 for s in samples)
    overlapping=[(s[4]-s[3])/1e6 for s in samples if any(s[3]<b and a<s[4] for a,b in periods)]
    slowest=max(samples,key=lambda s:s[4]-s[3])
    _,lane,_,start,end,retries=slowest
    coincident=sum(max(0,min(end,b)-max(start,a)) for a,b in periods)/1e6
    others=sum(s[1]!=lane and start<s[4]<end for s in samples)
    conflicts=sum(s[5] for s in samples)
    return dict(mode=mode,rep=run["rep"],tx_s=int(ops)*1e9/int(elapsed),
                p99_ms=lat[math.ceil(.99*len(lat))-1],max_ms=lat[-1],conflicts=conflicts,
                worst_checkpoint_overlap_ms=coincident,other_completions_during_worst=others,worst_retries=retries,overlap=len(overlapping),overlap_max_ms=max(overlapping),
                checkpoint_ms=median((b-a)/1e6 for a,b in periods),rss_MiB=int(rss)/2**20)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path)
    parser.add_argument("out",type=Path)
    parser.add_argument("--mvcc",action="store_true")
    parser.add_argument("--direct",action="store_true")
    args=parser.parse_args()
    rows=[summarize(args.raw,run) for run in json.loads((args.raw/"runs.json").read_text())]
    modes=("wal","mvcc") if args.mvcc else ("wal","dual","dirty")
    if args.direct:
        modes=("wal","mvcc","direct")
        for row in rows:
            log=(args.raw/f"{row['rep']}-{row['mode']}.log").read_text()
            samples=[line for line in log.splitlines() if "REOPEN_MEMORY," in line]
            assert len(samples)==1
            initial,at_ready,peak=map(int,samples[0].split("REOPEN_MEMORY,")[1].split(","))
            assert 0<initial<=peak and 0<at_ready<=peak
            row.update(reopen_start_MiB=initial/2**20,reopen_ready_MiB=at_ready/2**20,reopen_peak_MiB=peak/2**20)
    assert sorted((r["mode"],r["rep"]) for r in rows)==sorted((m,i) for m in modes for i in (1,2,3))
    args.out.mkdir(parents=True,exist_ok=True)
    with (args.out/"trials.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n")
        w.writeheader()
        w.writerows(rows)
    groups={m:[r for r in rows if r["mode"]==m] for m in modes}
    metrics=[("tx_s","Transactions / second"),("p99_ms","Request p99 (ms)"),
             ("max_ms","Worst request (ms)"),("checkpoint_ms","Checkpoint (ms)"),
             ("conflicts","Conflict retries / run"),("overlap","Requests overlapping a checkpoint")]
    names=("WAL","FIFO delta") if args.mvcc else ("WAL","Full dual","64 KiB delta")
    if args.direct:
        names=("WAL","Dual index","Direct")
        metrics=[("rss_MiB","End RSS (MiB)"),("reopen_peak_MiB","Reopen peak RSS (MiB)"),("tx_s","Transactions / second"),("p99_ms","Request p99 (ms)"),("max_ms","Worst request (ms)"),("checkpoint_ms","Checkpoint (ms)")]
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 420" role="img"><title>Four-writer checkpoint comparison</title><rect width="960" height="420" fill="white"/><g fill="#18293d" font-family="sans-serif">']
    def label(x,y,s,size):
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(16,30,"MVCC persistence: one million rows, four writers, two CPUs and 4 GiB" if args.mvcc else "One million rows, four writers, two CPUs and 4 GiB",23)
    label(16,55,"Median of three runs, except worst request across all runs. Each panel has an independent scale.",14)
    for i,(key,title) in enumerate(metrics):
        x,y=16+i%3*316,90+i//3*160
        label(x,y,title,16)
        values=[(max if key=="max_ms" else median)(r[key] for r in groups[m]) for m in modes]
        for j,(name,v,color) in enumerate(zip(names,values,("#396494","#a54c31","#28795d"))):
            yy=y+31+j*33
            label(x,yy,name,12)
            width=145*v/max(1,max(values))
            svg.append(f'<rect x="{x+70}" y="{yy-12}" width="{width:.2f}" height="13" fill="{color}"/>')
            label(x+75+width,yy,f"{v:,.2f}",12)
    (args.out/"comparison.svg").write_text("\n".join(svg+["</g></svg>"])+"\n",encoding="utf8")
    for m,group in groups.items():
        print(m,{k:round((max if k=="max_ms" else median)(r[k] for r in group),3) for k in group[0] if k not in ("mode","rep")})


if __name__=="__main__":
    main()
