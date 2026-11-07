"""Validate the twelve traced runs and produce paired, per-trial figures."""
import argparse
import csv
import json
import math
from pathlib import Path
from statistics import median
from xml.sax.saxutils import escape
from xml.etree import ElementTree

def load(path):
    with path.open(newline="") as f:return list(csv.DictReader(f))

def values(rows,m,v,key):
    return [float(r[key]) for r in sorted(rows,key=lambda r:int(r["rep"])) if (r["mode"],r["variant"])==(m,v)]

def figure(out,title,metrics,rows,label_aggregates=None):
    width=920;height=80+165*len(metrics)
    svg=[f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img"><title>{escape(title)}</title><desc>Three paired trials in each lock mode. Left point is baseline, right point is asynchronous 8 MiB writeback.</desc><rect width="100%" height="100%" fill="#fafafa"/><g font-family="system-ui,sans-serif" fill="#142f45">',
         f'<text x="20" y="31" font-size="23">{escape(title)}</text><text x="20" y="57" font-size="14">Each line connects the same trial number. Both lock panels use the same scale per row.</text>']
    for i,(description,key,unit) in enumerate(metrics):
        top=83+i*165
        high=max(float(r[key]) for r in rows) or 1
        svg.append(f'<text x="20" y="{top+10}" font-size="16">{escape(description)} ({unit})</text>')
        for j,(m,label) in enumerate([("standard","Current lock"),("fifo","FIFO")]):
            left=35+j*455;bottom=top+119
            svg.extend([f'<text x="{left}" y="{top+31}" font-size="13">{label}</text>',
                        f'<path d="M{left} {bottom}h380" stroke="#ced6df"/>'])
            for value,yy in ((0,bottom),(high/2,bottom-39),(high,bottom-78)):
                svg.append(f'<path d="M{left+51} {yy}h328" stroke="#e3e8eb"/><text x="{left+46}" y="{yy+3}" text-anchor="end" font-size="10">{value:,.1f}</text>')
            for idx,(a,b) in enumerate(zip(values(rows,m,"baseline",key),values(rows,m,"range8",key))):
                col=["#07638c","#b6552a","#635696"][idx];dash=["none","7 3","2 3"][idx];x1=left+94;x2=left+255
                y1=bottom-78*a/high;y2=bottom-78*b/high
                svg.append(f'<g><title>Trial {idx+1}: {a:.3f} to {b:.3f} {unit}</title><path d="M{x1} {y1}L{x2} {y2}" stroke="{col}" stroke-dasharray="{dash}" fill="none" stroke-width="2"/><circle cx="{x1}" cy="{y1}" r="3.5" fill="{col}"/><rect x="{x2-3}" y="{y2-3}" width="6" height="6" fill="{col}"/></g>')
            label_fn=(label_aggregates or {}).get(key,median)
            a=label_fn(values(rows,m,"baseline",key));b=label_fn(values(rows,m,"range8",key))
            svg.append(f'<text x="{left+94}" y="{bottom+18}" text-anchor="middle" font-size="13">Baseline {a:,.2f}</text><text x="{left+255}" y="{bottom+18}" text-anchor="middle" font-size="13">8 MiB {b:,.2f}</text>')
    out.write_text("".join(svg)+"</g></svg>",encoding="utf-8")
    ElementTree.parse(out)

def verify(rows,pubs,cap,reps):
    assert len(rows)==4*reps and len(pubs)==4*reps*cap
    assert len({(r["mode"],r["variant"],r["rep"]) for r in rows})==len(rows)
    for r in rows:
        ps=[p for p in pubs if all(p[k]==r[k] for k in ("variant","mode","rep"))]
        assert sorted(int(p["publication"]) for p in ps)==list(range(1,cap+1))
        assert all(p["app_B"]==p["write_returns_B"] for p in ps)
        assert sum(int(p["bio_B"]) for p in ps)==int(r["submitted_B"])
        assert sum(int(p["app_B"]) for p in ps)==int(r["logical_B"])==int(r["checkpoint_B"])
        assert math.isclose(median(int(p["peak10_B"])/2**20 for p in ps),float(r["peak10_MiB"]))
        assert math.isclose(100*sum(int(p["burst32_B"]) for p in ps)/int(r["submitted_B"]),float(r["bulk_pct"]))
        assert int(r["attributed"])+int(r["unattributed"])==int(r["updates"])

def main():
    parser=argparse.ArgumentParser();parser.add_argument("source",type=Path);args=parser.parse_args()
    d=args.source;rows=load(d/"trials.csv");pubs=load(d/"publications.csv")
    pilot=load(d/"pilot-trials.csv")
    verify(pilot,load(d/"pilot-publications.csv"),2,1);verify(rows,pubs,63,3)
    perf=[("Update throughput","updates_s","updates/s"),("Update p99","p99_ms","ms"),("Worst update per trial","max_ms","ms")]
    io=[("Peak in any rolling 10 ms, median across publications","peak10_MiB","MiB"),("Submitted bytes in bins of at least 32 MiB","bulk_pct","%"),
        ("WAL-targeted journal commit p99","journal_p99_ms","ms"),("Publication duration, median across publications","publish_ms","ms")]
    figure(d/"performance.svg","Update responsiveness and throughput",perf,rows)
    figure(d/"writeback.svg","Checkpoint writeback and WAL journal latency",io,rows)
    table=[]
    for label,key,unit in perf+io+[("Throughput including final drain","drained_s","updates/s"),("First prefix start through last publication","from_first_to_last_publication_s","s"),
                                   ("WAL-targeted journal commit median","journal_med_ms","ms"),("All application writes incl. WAL","write_MiB","MiB")]:
        parts=[f"{label} ({unit})"]
        for m in ("standard","fifo"):
            agg=max if key=="max_ms" else median
            a,b=[agg(values(rows,m,v,key)) for v in ("baseline","range8")]
            parts.extend([f"{a:,.2f}",f"{b:,.2f}",f"{100*(b/a-1):+.1f}%"])
        table.append("| "+" | ".join(parts)+" |")
    ratio=[]
    for m in ("standard","fifo"):
        for key in ("logical_B","submitted_B"):
            for a,b in zip(values(rows,m,"baseline",key),values(rows,m,"range8",key)):
                ratio.append(abs(b/a-1)*100)
    repeat=[]
    for m in ("standard","fifo"):
        counts=[]
        for key,sign in (("updates_s",1),("p99_ms",-1),("max_ms",-1),("journal_p99_ms",-1)):
            a=values(rows,m,"baseline",key);b=values(rows,m,"range8",key)
            counts.append(str(sum(sign*(y-x)>0 for x,y in zip(a,b)))+"/3")
        repeat.append("| "+m+" | "+" | ".join(counts)+" |")
    detail=[]
    for m in ("standard","fifo"):
        for rep in range(1,4):
            lookup={r["variant"]:r for r in rows if r["mode"]==m and int(r["rep"])==rep}
            cells=[m,str(rep)]
            for key in ("updates_s","p99_ms","max_ms","publish_ms","journal_p99_ms"):
                cells.append(" → ".join(f"{float(lookup[v][key]):,.1f}" for v in ("baseline","range8")))
            detail.append("| "+" | ".join(cells)+" |")
    worst_peak=[max(int(p["peak10_B"]) for p in pubs if p["variant"]==v)/2**20 for v in ("baseline","range8")]
    comparison=""
    if (d/"journal-groups.csv").exists():
        cohorts=load(d/"journal-groups.csv")
        def count(mode,variant,w):
            return sum(int(r["n"]) for r in cohorts if r["mode"]==mode and r["variant"]==variant
                       and r["window"]==w and r["exposure"]=="different_32_MiB_or_more")
        arms=[(m,v) for m in ("standard","fifo") for v in ("baseline","range8")]
        a,b,c,e=[count(m,v,"during") for m,v in arms]
        i,j,k,l=[count(m,v,"first5") for m,v in arms]
        comparison=(f"During WAL-targeted commits, epochs with at least 32 MiB submitted exclusively for other checkpoint targets fell from {a} to {b} with the current lock and from {c} to {e} with FIFO (three trials per arm). In the fixed first 5 ms of commits lasting at least 5 ms the counts were {i} to {j} and {k} to {l}. A long commit can still accumulate several 8 MiB ranges. The FIFO journal p99 and whole-update p99 can move in opposite directions because a request also waits for other writers and checkpoint publication.")
    report=f"""# Scheduling checkpoint writeback every 8 MiB on ext4

The baseline writes the replacement snapshot and synchronizes it before publication. The candidate additionally requests asynchronous writeback of each completed, nonoverlapping 8 MiB snapshot range. The same test executable supports both arms within each lock mode. The final snapshot sync, tail WAL sync, atomic replacement and directory sync retain their original ordering.

![Writeback and journal commit times](writeback.svg)

{comparison}

| Measure | Current baseline | Current 8 MiB | Change | FIFO baseline | FIFO 8 MiB | Change |
|---|---:|---:|---:|---:|---:|---:|
{chr(10).join(table)}

![Per-trial throughput and latency](performance.svg)

| Pairs improving with 8 MiB | Throughput | Update p99 | Worst update | Journal p99 |
|---|---:|---:|---:|---:|
{chr(10).join(repeat)}

The table uses three-run medians except for worst update, which is the maximum over all three trials. Figure labels are trial medians and each line joins one pair. Application write volume includes WAL plus the published replacement. The largest absolute paired difference in checkpoint logical or guest-submitted bytes was {max(ratio):.3f}%. Concurrent WAL suffix lengths vary with publication timing. Across all individual publications, the highest observed rolling 10 ms burst was {worst_peak[0]:.2f} MiB at baseline and {worst_peak[1]:.2f} MiB with range writeback. The header rewrite may dirty its first page again before final sync.

## Individual trials

Each cell shows baseline → 8 MiB.

| Lock | Pair | Updates/s | Update p99 ms | Worst update ms | Publication ms | WAL journal p99 ms |
|---|---:|---:|---:|---:|---:|---:|
{chr(10).join(detail)}

## Conditions and attribution

Each full run starts from a fresh Docker volume with one million 256-byte values on WSL2/ext4, four writers, two CPUs, 4 GiB, 16,384 durable updates and exactly 63 independent publications. The condition order reverses in the second round. Comparisons hold the observer, update stream and publication count fixed. Guest submissions use the prior ext4_end_bio and checkpoint inode lifetime filter. Publication duration starts when the prepared prefix is handed to the final checkpoint path and ends when the call returns. The separate first-prefix-to-last-publication metric includes the intervening workload. The peak for each publication is the largest sum in any rolling 10 ms interval. Large-burst share counts bytes in fixed monotonic-clock 10 ms bins with at least 32 MiB for that publication. The journal figures measure JBD2 start to end for WAL-targeted commits with verified waits. These two endpoints differ from whole-request latency. [Prior attribution procedure](../ext4-checkpoint-bytes-2026-11-08/README.md).

The pilot ran 768 updates and two publications per arm. Before full runs, both lock modes had to reduce the median per-publication peak by at least 20%, reduce the large-burst share, and keep submitted bytes within 2%. The pilot passed. [Pilot trials](pilot-trials.csv) and [pilot publications](pilot-publications.csv).

## Safety and reproducibility

The range hint applies to the unpublished snapshot in the 64-bit Linux test build. Linux may block a range request under queue pressure. The final fsync flushes the last partial range and supplies durability. [Linux sync_file_range documentation](https://man7.org/linux/man-pages/man2/sync_file_range.2.html). Submission errors propagate and abandon the unpublished checkpoint. All measured runs check retained reads, committed generation order, final values and reopened contents. Results describe this traced guest and device.

Build from the repository root with `docker build -f benchmarks/scaled-gate/Dockerfile.fifo -t catdb-range-writeback .` and the previous observer image `catdb-fifo-products`. Run `python benchmarks/range-writeback/campaign.py FRESH_RAW_DIR REPORT_DIR`, then `python benchmarks/range-writeback/exposure.py FRESH_RAW_DIR/full REPORT_DIR`, `python benchmarks/range-writeback/report_range.py REPORT_DIR` and `python benchmarks/range-writeback/verify_range.py REPORT_DIR --raw FRESH_RAW_DIR/full`. [All twelve trials](trials.csv) · [All 756 publications](publications.csv).
"""
    (d/"README.md").write_text(report,encoding="utf-8")
    print("\n".join(table))
if __name__=="__main__": main()
