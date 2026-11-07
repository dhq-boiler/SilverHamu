"""Reconcile every prewritten byte with the page-file counters for the rotated workload."""
import argparse
import csv
import json
import re
from pathlib import Path
from statistics import median
from report_four import summarize

MODES = ("arrivals", "prewrite", "defer")
NAMES = ("Scan", "Prewrite", "Defer")
CATEGORIES = ("published", "invalidated", "split", "failed", "pending")

def records(log, tag):
    return [tuple(map(int, line.split(","))) for line in
            re.findall(tag + r",([\d,]+)", log)]

def write_csv(path, rows):
    with path.open("w", newline="") as f:
        w = csv.DictWriter(f, rows[0], lineterminator="\n")
        w.writeheader()
        w.writerows(rows)

def report(raw, out):
    out.mkdir(parents=True, exist_ok=True)
    runs = json.loads((raw / "runs.json").read_text())
    assert sorted((r["mode"],r["rep"]) for r in runs) == sorted((m,i) for m in MODES for i in (1,2,3))
    trials = []
    for r in runs:
        row = summarize(raw, r)
        log = (raw / (r["label"] + ".log")).read_text()
        stages = records(log, "STAGE_BYTES")
        assert stages and all(len(s)==5 for s in stages)
        amounts = list(map(sum,zip(*stages)))
        totals = records(log,"PREWRITE_TOTAL")
        assert len(totals) == int(r["mode"]!="arrivals")
        assert sum(amounts) == (totals[0][1] if totals else 0)
        cp = [c for c in records(log,"PREWRITE_CHECKPOINT") if c[0]]
        assert [c[0] for c in cp] == [256,512,768]
        final = sum(c[2] for c in cp)
        row.update(zip(CATEGORIES,amounts))
        row.update(pages_reused=sum(c[1] for c in cp), final_bytes=final,
                   total_MiB=(sum(amounts)+final)/2**20)
        trials.append(row)
    write_csv(out/"trials.csv",trials)
    summary=[]
    for m in MODES:
        rows=[r for r in trials if r["mode"]==m]
        summary.append(dict(mode=m,**{k:(max if k=="max_ms" else median)(r[k] for r in rows)
                        for k in ("total_MiB","p99_ms","max_ms","tx_s","rss_MiB","checkpoint_ms",*CATEGORIES)}))
    write_csv(out/"summary.csv",summary)
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 870 390" role="img"><title>Deferring invalidated ranges in the four-writer benchmark</title><rect width="870" height="390" fill="white"/><g font-family="sans-serif" fill="#192b40">']
    def label(x,y,s,size=14):
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(16,30,"Deferring invalidated checkpoint ranges",23)
    label(16,55,"One million rows, four writers. Median of three trials except for the worst update.")
    for i,(key,title) in enumerate([("total_MiB","Page writes (MiB)"),("p99_ms","Update p99 (ms)"),("max_ms","Worst update (ms)"),("tx_s","Updates per second")]):
        x,y=16+(i%2)*430,88+(i//2)*150
        label(x,y,title,17)
        peak=max(r[key] for r in summary)
        for j,(r,name,color) in enumerate(zip(summary,NAMES,("#596e86","#ae5d35","#267264"))):
            yy=y+26+j*29
            label(x,yy,name);width=190*r[key]/peak
            svg.append(f'<rect x="{x+70}" y="{yy-12}" width="{width:.2f}" height="14" fill="{color}"/>')
            label(x+width+78,yy,f"{r[key]:,.2f}")
    (out/"comparison.svg").write_text("\n".join(svg+["</g></svg>"]))
if __name__=="__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    report(args.raw,args.out)
