"""Summarize durable writer traffic and response times between completed checkpoints."""
import argparse
import csv
import gzip
import json
import math
import re
from pathlib import Path
from statistics import median

MODES = ("arrivals","prewrite","defer")
LABELS = ("Publish-time scan","Immediate prewrite","Defer invalidated")
OPS = 16384
CAPACITY = 256

def records(text, tag):
    return [tuple(map(int,s.split(","))) for s in re.findall(tag+r",([0-9,]+)",text)]

def write_csv(path, rows):
    with path.open("w", newline="") as f:
        writer=csv.DictWriter(f,rows[0],lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)

def latency(values, q):
    return sorted(values)[math.ceil(q*len(values))-1]/1e6

def analyze(text,mode,rep):
    cp=records(text,"FOUR_CHECKPOINT")
    counts=records(text,"WINDOW_BYTES")
    seqs=list(range(CAPACITY,OPS,CAPACITY))
    assert [r[0] for r in cp]==list(range(len(seqs)))
    assert [r[0] for r in counts]==[0]+seqs
    boundaries={v:(w[2],*c[1:]) for v,w,c in zip(seqs,cp,counts[1:])}
    requests=[tuple(map(int,s.split(","))) for s in re.findall("SCALED_REQUEST,"+mode+r",([0-9,]+)",text)]
    assert sorted(r[0] for r in requests)==list(range(OPS))
    assert sorted(r[2] for r in requests)==list(range(1,OPS+1))
    assert all(r[3]<=r[4] and r[1]==r[0]%4 for r in requests)
    cohort=records(text,"CARRY_OUTCOME")
    assert len(cohort)==1
    at,marked,published,invalidated,survivors=cohort[0]
    assert at==1024 and marked==published+invalidated
    def interval(start,end):
        a,p0,w0,i0=boundaries[start]
        b,p1,w1,i1=boundaries[end]
        assert b>a and w1-w0>=p1-p0>=0 and i1>=i0
        completed=[r for r in requests if a<r[4]<=b]
        lat=[r[4]-r[3] for r in completed]
        return dict(first=start,last=end,commits=end-start,responses=len(lat),
                    page_B_per_commit=(p1-p0)/(end-start),store_B_per_commit=(w1-w0)/(end-start),
                    invalidated_B_per_commit=(i1-i0)/(end-start),
                    p99_ms=latency(lat,.99),max_ms=max(lat)/1e6,
                    commits_s=(end-start)*1e9/(b-a))
    steady=interval(1024,seqs[-1])
    chunks=[dict(mode=mode,rep=rep,**interval(i,i+CAPACITY)) for i in range(1024,seqs[-1],CAPACITY)]
    assert sum(c["responses"] for c in chunks)==steady["responses"]
    return dict(mode=mode,rep=rep,**steady,marked_B=marked,published_B=published,
                invalidated_marked_B=invalidated,published_slots=survivors,
                full_max_ms=max(r[4]-r[3] for r in requests)/1e6),chunks

def chart(path,summary):
    out=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 410" role="img"><title>Steady checkpoint intervals, four writers</title><rect width="960" height="410" fill="white"/><g font-family="sans-serif" fill="#182b40">']
    def label(x,y,text,size):
        out.append(f'<text x="{x}" y="{y}" font-size="{size}">{text}</text>')
    label(16,32,"Four writers across 59 complete checkpoint intervals",24)
    label(16,56,"One million rows, two CPUs, 4 GiB. Three trial medians, maximum across trials for worst latency.",15)
    for i,(key,title) in enumerate([("page_B_per_commit","Page-file bytes per commit"),("p99_ms","Update p99 (ms)"),("max_ms","Worst update (ms)"),("commits_s","Commits per second")]):
        x,y=16+i%2*474,93+i//2*155
        label(x,y,title,18)
        high=max(r[key] for r in summary)
        for j,(r,name,color) in enumerate(zip(summary,LABELS,("#566c82","#a35436","#237463"))):
            yy=y+26+j*30
            label(x,yy,name,14)
            width=190*r[key]/high
            out.append(f'<rect x="{x+148}" y="{yy-12}" width="{width:.2f}" height="14" fill="{color}"/>')
            label(x+155+width,yy,f"{r[key]:,.2f}",14)
    path.write_text("\n".join(out+["</g></svg>",""]))

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    args.out.mkdir(parents=True,exist_ok=True)
    runs=json.loads((args.raw/"runs.json").read_text())
    assert sorted((r["mode"],r["rep"]) for r in runs)==sorted((m,i) for m in MODES for i in (1,2,3))
    trials=[];windows=[]
    for r in runs:
        assert r["status"]=="passed" and not r["state"]["OOMKilled"] and r["summary"].endswith(",verified")
        path=args.raw/(r["label"]+".log")
        text=path.read_text() if path.exists() else gzip.decompress(path.with_suffix(".log.gz").read_bytes()).decode()
        trial,parts=analyze(text,r["mode"],r["rep"])
        trials.append(trial);windows.extend(parts)
    summary=[]
    for m in MODES:
        selected=[r for r in trials if r["mode"]==m]
        summary.append(dict(mode=m,**{k:(max if k in ("max_ms","full_max_ms") else median)(r[k] for r in selected)
                                      for k in ("page_B_per_commit","store_B_per_commit","invalidated_B_per_commit","p99_ms","max_ms","commits_s","full_max_ms")}))
    write_csv(args.out/"trials.csv",trials)
    write_csv(args.out/"windows.csv",windows)
    write_csv(args.out/"summary.csv",summary)
    chart(args.out/"steady.svg",summary)
if __name__=="__main__":
    main()
