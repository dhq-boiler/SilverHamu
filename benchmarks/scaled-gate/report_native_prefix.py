"""Measure successful logical WAL writes across adjacent completed publications."""
import argparse
import csv
import gzip
import json
import math
import re
from pathlib import Path
from statistics import median

MODES=("prefix-off","prefix-on")
OPS=16_384
PERIOD=256
HEADER=48

def tags(text,tag):
    return [tuple(map(int,s.split(","))) for s in re.findall(tag+r",([0-9,]+)",text)]

def quantile(ns):
    return sorted(ns)[math.ceil(.99*len(ns))-1]/1e6

def write(path,rows):
    with path.open("w",newline="") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n")
        w.writeheader()
        w.writerows(rows)

def parse(text,mode,rep):
    windows=tags(text,"FOUR_CHECKPOINT")
    files=tags(text,"NATIVE_BYTES")
    samples=tags(text,"SCALED_REQUEST,"+mode)
    assert [i for i,a,b in windows]==list(range(63)) and len(files)==63
    assert all(b>a for i,a,b in windows)
    assert sorted(s[0] for s in samples)==list(range(OPS))
    seq=sorted(s[2] for s in samples)
    assert seq==list(range(seq[0],seq[0]+OPS))
    assert all(s[1]==s[0]%4 and s[4]>=s[3] for s in samples)
    periods=[]
    for j in range(4,63):
        _,started,t1=windows[j]
        t0=windows[j-1][2]
        old_end,base,after,prefix=files[j]
        old_file=files[j-1][2]
        assert t1>started>=t0 and old_file<=base<=old_end and prefix<=after
        assert (prefix>0)==(mode=="prefix-on")
        # after includes the copied tail and initial header but excludes the header rewrite.
        appends=old_end-old_file
        copied=old_end-base
        snapshot=after-copied
        total=appends+after+HEADER
        data=[s[4]-s[3] for s in samples if t0<s[4]<=t1]
        periods.append(dict(mode=mode,rep=rep,first_commit=j*PERIOD,last_commit=(j+1)*PERIOD,
                            responses=len(data),elapsed_ns=t1-t0,total_B=total,append_B=appends,
                            copied_tail_B=copied,snapshot_B=snapshot,header_rewrite_B=HEADER,
                            prefix_B=prefix,checkpoint_ms=(t1-started)/1e6,p99_ms=quantile(data),max_ms=max(data)/1e6))
    completed=[s for s in samples if windows[3][2]<s[4]<=windows[-1][2]]
    window=[s[4]-s[3] for s in completed]
    p99_request=sorted(completed,key=lambda r:r[4]-r[3])[math.ceil(.99*len(completed))-1]
    def overlap(request):
        a,b=request[3:5]
        return sum(max(0,min(b,end)-max(a,start)) for _,start,end in windows)
    quiet=[s[4]-s[3] for s in completed if overlap(s)==0]
    duration=windows[-1][2]-windows[3][2]
    n=len(periods)*PERIOD
    assert n==15104 and sum(p["responses"] for p in periods)==len(window)
    row=dict(mode=mode,rep=rep,commits=n,responses=len(window),KiB_per_commit=sum(p["total_B"] for p in periods)/1024/n,
             p99_ms=quantile(window),max_ms=max(window)/1e6,commits_s=n*1e9/duration,
             checkpoint_ms=median(p["checkpoint_ms"] for p in periods),
             prefix_MiB_per_checkpoint=median(p["prefix_B"] for p in periods)/2**20,
             tail_MiB_per_checkpoint=median(p["copied_tail_B"] for p in periods)/2**20,
             p99_request_checkpoint_overlap_ms=overlap(p99_request)/1e6,
             no_checkpoint_overlap_p99_ms=quantile(quiet),
             full_max_ms=max(s[4]-s[3] for s in samples)/1e6)
    return row,periods

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path);parser.add_argument("out",type=Path)
    a=parser.parse_args();a.out.mkdir(exist_ok=True,parents=True)
    runs=json.loads((a.raw/"runs.json").read_text())
    assert sorted((r["mode"],r["rep"]) for r in runs)==sorted((m,i) for m in MODES for i in (1,2,3))
    trials=[];intervals=[]
    for r in runs:
        assert r["status"]=="passed" and not r["state"]["OOMKilled"] and r["summary"].endswith(",verified")
        p=a.raw/(r["label"]+".log")
        text=p.read_text() if p.exists() else gzip.decompress(p.with_suffix(".log.gz").read_bytes()).decode()
        trial,rows=parse(text,r["mode"],r["rep"]);trials.append(trial);intervals.extend(rows)
    write(a.out/"trials.csv",trials);write(a.out/"intervals.csv",intervals)
    summary=[]
    for m in MODES:
        group=[r for r in trials if r["mode"]==m]
        summary.append(dict(mode=m,**{k:(max if k in ("max_ms","full_max_ms") else median)(r[k] for r in group)
                            for k in trials[0] if k not in ("mode","rep","commits","responses")}))
    write(a.out/"summary.csv",summary)
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 920 360" role="img"><title>Production WAL checkpoint prefix</title><rect width="920" height="360" fill="white"/><g font-family="sans-serif" fill="#182b40">']
    def label(x,y,s,size):
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(18,30,"Production WAL checkpoint with ahead-of-time prefix writes",23)
    label(18,54,"One million rows, four writers, 59 complete periods per trial. Maximum across trials for worst wait.",14)
    for i,(k,title) in enumerate([("KiB_per_commit","Logical WAL writes (KiB/commit)"),("commits_s","Commits per second"),("p99_ms","Update p99 (ms)"),("max_ms","Worst update (ms)")]):
        x,y=18+i%2*455,91+i//2*128; label(x,y,title,17)
        for j,(r,name,color) in enumerate(zip(summary,["Off","On"],["#576e83","#237465"])):
            yy=y+31+j*33;width=220*r[k]/max(v[k] for v in summary)
            label(x,yy,name,14)
            svg.append(f'<rect x="{x+45}" y="{yy-12}" width="{width:.2f}" height="14" fill="{color}"/>')
            label(x+52+width,yy,f"{r[k]:,.2f}",14)
    (a.out/"comparison.svg").write_text("\n".join(svg+["</g></svg>",""]))
if __name__=="__main__":main()
