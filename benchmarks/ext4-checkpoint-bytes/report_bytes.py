"""Summarize the checkpoint writeback data in three observation windows."""
import argparse
import bisect
import csv
import gzip
import json
import math
import sys
from collections import defaultdict
from pathlib import Path
from statistics import mean, median
from join import BENCH, run
from identity import analyze
from dose import dose_plot

sys.path.insert(0,str(BENCH/"journal-identity"))
from report import chart, save

def exposure(row,window):
    if row[window+"_same_B"] or row[window+"_unknown_B"]: return "mixed/same/unknown"
    other=row[window+"_other_B"]
    if not other:return "zero"
    return "other <1 MiB" if other<2**20 else "other >=1 MiB"

def local_controls(rows, broad=False):
    controls=defaultdict(list)
    for r in sorted(rows,key=lambda x:x["start_ns"]):
        if exposure(r,"prior")=="zero":
            controls[r["publication"],r["phase"]].append((r["start_ns"],r["duration_us"]))
    pairs=[]
    for r in rows:
        if not (r["prior_other_B"] if broad else exposure(r,"prior").startswith("other")):continue
        values=controls[r["publication"],r["phase"]]
        j=bisect.bisect_left(values,(r["start_ns"],))
        near=[v for v in values[max(0,j-1):j+1] if abs(r["start_ns"]-v[0])<=1_000_000_000]
        if near:
            t,d=min(near,key=lambda v:abs(r["start_ns"]-v[0]))
            pairs.append(dict(tx=r["tx"],control_start_ns=t,gap_ms=abs(t-r["start_ns"])/1e6,
                              delta_ms=(r["duration_us"]-d)/1000))
    return pairs

def med(v):
    return median(v) if v else ""

def summarize(folder,trial,out,pilot,groups,pubs,quality):
    publications,rows,stats=run(folder)
    meta=json.loads((folder/"meta.json").read_text())
    key={"mode":trial["mode"],"rep":trial["rep"]}
    quality.append(dict(**key,kernel=meta["kernel"],device=meta["device"],buffers=len(meta["stats"]),
                        max_probe_misses=max(int(l.split()[-1]) for l in meta["probe_profile"]),ring_loss=0,cleaned=meta["cleaned"]))
    for p in publications:
        assert abs(p["app_B"]-p["bio_B"])<=max(128*1024,.02*p["app_B"]),p
        pubs.append(dict(**key,**p))
    trial.update(epochs=len(rows),wal_resolved=stats["wal_resolved"],wal_total=stats["wal"],
                 pub_app_B=sum(p["app_B"] for p in publications),pub_bio_B=sum(p["bio_B"] for p in publications))
    for window in ("prior","during","first5"):
        by=defaultdict(list)
        broad=[];any_bytes=[]
        for r in rows:
            if window=="first5" and r["remaining_at_5ms_us"]=="": continue
            ms=r["remaining_at_5ms_us" if window=="first5" else "duration_us"]/1000
            label=exposure(r,window)
            by[label].append(ms)
            if r[window+"_other_B"]:broad.append(ms)
            if label!="zero":any_bytes.append(ms)
        for label,v in sorted(by.items()):
            groups.append(dict(**key,window=window,exposure=label,n=len(v),mean_ms=mean(v),
                               median_ms=median(v),p99_ms=sorted(v)[math.ceil(.99*len(v))-1],max_ms=max(v)))
        for label,values in (("zero",by["zero"]),("other",by["other <1 MiB"]+by["other >=1 MiB"]),("any_other",broad),("any",any_bytes)):
            trial[window+"_"+label+"_n"]=len(values)
            trial[window+"_"+label+"_med"]=med(values)
    for rname,broad in (("strict",False),("broad",True)):
        pairs=local_controls(rows,broad)
        trial[rname+"_matched"]=len(pairs)
        trial[rname+"_controls"]=len({p["control_start_ns"] for p in pairs})
        trial[rname+"_delta_ms"]=med([p["delta_ms"] for p in pairs])
        if pairs and not pilot:save(out/(folder.name+"-"+rname+"-pairs.csv"),pairs)
    if not pilot:
        with gzip.open(out/(folder.name+"-epochs.csv.gz"),"wt",newline="") as f:
            w=csv.DictWriter(f,rows[0].keys(),lineterminator="\n");w.writeheader();w.writerows(rows)

def collect(root,out,pilot):
    env=json.loads((root/"environment.json").read_text())
    trials=[];groups=[];pubs=[];quality=[]
    for folder in sorted(p for p in root.iterdir() if p.is_dir()):
        rep,mode,trace=folder.name.split("-")
        trial,_,_=analyze((folder/"app.log").read_text(),mode,int(rep),env["ops"],env["cap"])
        assert trial["checkpoints"]==trial["triggers"] and not trial["coalesced"]
        trial["trace"]=trace
        if trace=="trace": summarize(folder,trial,out,pilot,groups,pubs,quality)
        trials.append(trial)
    prefix="pilot-" if pilot else ""
    save(out/(prefix+"trials.csv"),[{k:r.get(k,"") for k in max(trials,key=len)} for r in trials])
    for name,data in (("groups",groups),("publications",pubs),("quality",quality)):save(out/(prefix+name+".csv"),data)
    return trials

def fmt(x):return f"{x:,.3f}" if isinstance(x,(int,float)) else "n/a"

def main():
    parser=argparse.ArgumentParser()
    for k in ("pilot","full","out"):parser.add_argument(k,type=Path)
    args=parser.parse_args();out=args.out;out.mkdir(parents=True,exist_ok=True)
    short=collect(args.pilot,out,True);rows=collect(args.full,out,False)
    assert len(rows)==6 and len(short)==8
    dose_plot(out)
    chart(out/"performance.svg","Independent checkpoints on ext4",[
        ("Updates to last response, median trial","updates_s","updates/s",median),
        ("Update p99, median trial","p99_ms","ms",median),("Worst update across three trials","max_ms","ms",max)],rows)
    def two(k,fn=median):
        groups=([r[k] for r in rows if r["mode"]==m] for m in ("standard","fifo"))
        return [fn(v) if all(isinstance(x,(float,int)) for x in v) else None for v in groups]
    metrics=[("Updates/s","updates_s",median),("Update p99, ms","p99_ms",median),("Worst update, ms","max_ms",max),
             ("WAL waits attributed","wal_resolved",sum)]
    for w,title in (("prior","10 ms before start"),("during","During commit"),("first5","First 5 ms among surviving commits")):
        for k,label in (("zero","zero"),("other","only different targets"),("any_other","any proven different targets"),("any","any checkpoint bytes")):
            metrics.extend([(title+", "+label+", count",w+"_"+k+"_n",sum),
                            (title+", "+label+", median ms",w+"_"+k+"_med",median)])
    for k in ("strict","broad"):
        metrics.extend([(k+" local matches",k+"_matched",sum),(k+" local paired difference, ms",k+"_delta_ms",median)])
    table=["| "+label+" | "+" | ".join(fmt(x) for x in two(k,fn))+" |" for label,k,fn in metrics]
    prior=two("prior_other_n",sum);during=two("during_other_n",sum)
    d0=two("during_zero_med");d1=two("during_other_med")
    lead=f"Of the WAL-targeted ext4 commits, {during[0]:,} standard and {during[1]:,} FIFO epochs overlapped submissions exclusively targeting other transactions. Their median durations were {fmt(d1[0])} and {fmt(d1[1])} ms, versus {fmt(d0[0])} and {fmt(d0[1])} ms with zero checkpoint bytes. The fixed 10 ms window before commit start showed exclusively different targets for {prior[0]:,} and {prior[1]:,} epochs respectively."
    overhead=[]
    for m in ("standard","fifo"):
        for k in ("updates_s","p99_ms","max_ms"):
            a,b=[mean(r[k] for r in short if r["trace"]==v and r["mode"]==m) for v in ("plain","trace")]
            overhead.append(f"| {m} | {k} | {a:.2f} | {b:.2f} | {(b/a-1)*100:+.1f}% |")
    with (out/"volume.csv").open() as f:
        volumes={(r["mode"],r["rep"],r["exposure"]):float(r["median_ms"]) for r in csv.DictReader(f) if r["window"]=="during"}
    comparisons=[(volumes[m,r,"32+ MiB"],volumes[m,r,"8-32 MiB"]) for m,r,k in volumes if k=="32+ MiB" and (m,r,"8-32 MiB") in volumes]
    more=sum(high>mid for high,mid in comparisons)
    zero=two("first5_zero_med");different=two("first5_other_med")
    gaps=[fmt(b-a) if a is not None and b is not None else "n/a" for a,b in zip(zero,different)]
    dose_summary=(f"The 32 MiB or larger group had a higher median duration than the 8 to under 32 MiB group in {more} of {len(comparisons)} comparable trials. These finer bins are exploratory. At the 5 ms landmark, remaining duration with exclusively different-target bytes minus the zero-byte group was {gaps[0]} ms for standard and {gaps[1]} ms for FIFO. [Writeback volume and duration by trial](volume.csv).")
    doc=f"""# Checkpoint data volume around ext4 journal commits

{lead} Longer commits allow more opportunities for concurrent writeback. These observational differences alone cannot establish the delay caused by a checkpoint.

![Per-trial journal duration by checkpoint data volume](dose.svg)

{dose_summary}

Each lock ran three times on one million 256-byte values, four writers, two CPUs, 4 GiB, 16,384 acknowledged updates and 63 independently published prefix checkpoints. The pair order was standard/FIFO, FIFO/standard, standard/FIFO. Each run checked the retained reader, latest values, commit sequence and reopened data. Database persistence and engine code follow the previous journal identity experiment.

![Update latency and throughput](performance.svg)

| Measure | Standard | FIFO |
|---|---:|---:|
{chr(10).join(table)}

Counts sum three runs. Durations and throughput are medians of trial medians or trial rates; maximum is the worst update across the three. An unavailable trial median makes the three-trial median n/a. The first-5-ms outcome measures remaining time after 5 ms. Local comparisons match the closest zero-exposure commit within one second in the same publication and phase. A control can serve several exposed commits. Per-trial counts of distinct controls are in trials.csv.

## Measuring checkpoint bytes

The checkpoint thread's successful write return sizes must match the application's total for every publication. A private probe at submit_bio filters ext4_end_bio on the workload device. It reads BIO size, inode identity and the inode fsync target i_sync_tid using the running kernel's BTF layout. The checkpoint inode is identified at snapshot fsync, then followed from prefix preparation through final data sync; after replacement it can become the active WAL. The counts include background workers. BIO sizes measure guest data submissions and include page rounding or rewrites. They omit metadata and cannot measure the physical drive. [Linux ext4 data submission](https://github.com/torvalds/linux/blob/v6.18/fs/ext4/page-io.c).

An initial large trial probing ext4_io_submit saw 6.91 GB versus 17.98 GB of logical writes. Its performance data were discarded. The replacement probe calibrated on two million-row publications measured 570,814,464 submitted bytes versus 570,799,758 logical bytes. Each retained publication must reconcile within 2% or 128 KiB, whichever is larger.

## Matching a journal commit

Each WAL wait must match a unique start/end event for the device and transaction and have a verified JBD2 sleep. Repeated WAL waiters count once per epoch. For each data BIO, compare the inode target at submission and the snapshot/tail fsync target to the WAL ID. Both equal means same; both known and different from WAL means different. The data separates contradictory and missing targets. This is fsync-target attribution; individual data BIOs have no JBD2 ownership field.

The primary exposure is the half-open 10 ms before commit start. During-commit counts are also shown with two byte-volume bins. Preceding windows can overlap, so only per-publication sums measure total traffic. The broad category includes any proven different-target bytes even when other bytes are ambiguous. That sensitivity analysis and a fixed 5 ms landmark were added after observing sparse exclusive exposure in the first full trial. The landmark compares remaining duration only for commits still active at 5 ms. Phase, load and previous I/O can affect both exposure and commit duration.

## Observer pilot

Two 10,000-row trials of 512 updates and 15 publications per mode and observer setting. Differences in these short means include ordinary workload variability. All retained traces must pass ring drop, probe miss and cleanup checks.

| Mode | Measure | Plain mean | Traced mean | Change |
|---|---|---:|---:|---:|
{chr(10).join(overhead)}

[All six runs](trials.csv) · [Exposure groups and sample sizes](groups.csv) · [All publication byte counts](publications.csv) · [Pilot trials](pilot-trials.csv) · [Capture checks](quality.csv). Per-run compressed tables contain one row per WAL epoch. Build the two images from the [journal identity experiment](../journal-identity-2026-11-07/README.md). Use the [new collector](../../../benchmarks/ext4-checkpoint-bytes/) to run run.py --pilot PILOT, then run.py FULL, report_bytes.py PILOT FULL REPORT and verify_bytes.py REPORT on fresh directories.
"""
    (out/"README.md").write_text(doc,encoding="utf-8");print(lead);print("\n".join(table))
if __name__=="__main__":main()
