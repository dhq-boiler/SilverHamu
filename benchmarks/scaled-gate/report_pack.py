"""Summarize the checkpoint copy experiment using raw request and phase records."""
from pathlib import Path
from statistics import median
import argparse
import csv
import json
import math
from report_four import summarize

MODES=("mvcc","direct","packed")
NAMES=("Dual index","Scan page","Pack range")

def write(path,rows):
    with path.open("w",newline="",encoding="utf8") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)

def overlap_rows(raw,run):
    requests=[]
    windows=[]
    for line in (raw/(run["label"]+".log")).read_text().splitlines():
        fields=line.split(",")
        if fields[0]=="FOUR_CHECKPOINT":windows.append(tuple(map(int,fields[2:])))
        if fields[0]=="SCALED_REQUEST":requests.append(tuple(map(int,fields[2:])))
    assert len(windows)==3 and len(requests)==1024
    timed=sorted((s[4]-s[3],sum(max(0,min(s[4],b)-max(s[3],a)) for a,b in windows)) for s in requests)
    quiet=sorted(dt for dt,overlap in timed if overlap==0)
    p99=timed[math.ceil(.99*len(timed))-1]
    return dict(p99_overlap_ms=p99[1]/1e6,max_overlap_ms=timed[-1][1]/1e6,
                outside_only_p99_ms=quiet[math.ceil(.99*len(quiet))-1]/1e6)

def phase_rows(raw,run):
    text=(raw/(run["label"]+".log")).read_text()
    events=[list(map(int,l.split("CHECKPOINT_PHASE,")[1].split(","))) for l in text.splitlines() if "CHECKPOINT_PHASE," in l]
    windows=[list(map(int,l.split(",")[2:])) for l in text.splitlines() if l.startswith("FOUR_CHECKPOINT,")]
    phases=[e for e in events if e[0] in (256,512,768)]
    assert [e[0] for e in phases]==[256,512,768] and len(windows)==3
    labels=("other_ms","scan_ms","assemble_ms","write_ms","sync_ms")
    rows=[]
    for (seq,*ns),(a,b) in zip(phases,windows):
        assert len(ns)==5 and sum(ns)<=b-a
        row=dict(mode=run["mode"],sequence=seq,checkpoint_ms=(b-a)/1e6,
                 outside_markers_ms=(b-a-sum(ns))/1e6)
        row.update(zip(labels,(v/1e6 for v in ns)));rows.append(row)
    return rows

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path);parser.add_argument("out",type=Path)
    parser.add_argument("--reuse",action="store_true")
    parser.add_argument("--linear",action="store_true")
    parser.add_argument("--combine",action="store_true")
    args=parser.parse_args();args.out.mkdir(parents=True,exist_ok=True)
    modes=MODES+(("reuse",) if args.reuse else ())
    names=NAMES+(("Pack + reuse",) if args.reuse else ())
    if args.linear:
        modes=("reuse","linear");names=("Page map","Linear")
    if args.combine:
        modes=("linear","combine");names=("Full scan","Combine")
    groups={}
    for stage in ("diagnostic","1m"):
        raw=args.raw/stage
        runs=json.loads((raw/"runs.json").read_text())
        assert sorted((r["mode"],r["rep"]) for r in runs)==sorted((m,i) for m in modes for i in (range(1,4) if stage=="1m" else [1]))
        rows=[]
        for r in runs:
            v=summarize(raw,r)
            v.update(overlap_rows(raw,r));rows.append(v)
        write(args.out/(stage+".csv"),rows)
        if stage=="diagnostic" and args.combine:
            counters=[]
            for r in runs:
                file=raw/(r["label"]+".log")
                entries=[tuple(map(int,line.split("PAGE_CRC,")[1].split(","))) for line in file.read_text().splitlines() if "PAGE_CRC," in line]
                entries=[e for e in entries if e[0] in (256,512,768)]
                assert [e[0] for e in entries]==[256,512,768]
                for (seq,inner,outer),phase in zip(entries,phase_rows(raw,r)):
                    assert (inner+outer)/1e6 <= phase["assemble_ms"]
                    counters.append(dict(mode=r["mode"],sequence=seq,body_ms=inner/1e6,outer_ms=outer/1e6))
            write(args.out/"crc.csv",counters)
        if stage=="diagnostic":write(args.out/"phases.csv",[v for r in runs for v in phase_rows(raw,r)])
        else:groups={m:[r for r in rows if r["mode"]==m] for m in modes}
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 460" role="img"><title>Checkpoint scan and copy comparison</title><rect width="960" height="460" fill="white"/><g fill="#17283b" font-family="sans-serif">']
    def label(x,y,s,size):svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(16,32,"MVCC checkpoint reads: one million rows and four writers",23)
    label(16,56,"Two CPUs, 4 GiB. Trial medians except worst request across all trials. Independent scales.",14)
    metrics=[("p99_ms","Update p99 (ms)"),("max_ms","Worst update (ms)"),("tx_s","Transactions / second"),
             ("rss_MiB","End RSS (MiB)"),("checkpoint_ms","Checkpoint (ms)"),("outside_only_p99_ms","p99 outside checkpoints (ms)")]
    for i,(key,title) in enumerate(metrics):
        x,y=16+i%3*316,88+i//3*184;label(x,y,title,17)
        vals=[(max if key=="max_ms" else median)(r[key] for r in groups[m]) for m in modes]
        for j,(name,v,color) in enumerate(zip(names,vals,("#526c88","#a54c31","#27785e","#594885"))):
            yy=y+29+33*j;label(x,yy,name,12);width=142*v/max(vals)
            svg.append(f'<rect x="{x+72}" y="{yy-12}" width="{width:.2f}" height="13" fill="{color}"/>')
            label(x+78+width,yy,f"{v:,.2f}",12)
    (args.out/"comparison.svg").write_text("\n".join(svg+["</g></svg>"])+"\n")
if __name__=="__main__":main()
