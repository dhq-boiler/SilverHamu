"""Export numeric commit-attempt markers and verify the nine runs and the earlier twelve."""
import argparse
import csv
import gzip
import json
import math
import re
import os
import shutil
from pathlib import Path
from statistics import median
from analyze_overtakes import summarize_attempts

def load(path):
    with path.open(newline="") as stream:return list(csv.DictReader(stream))

def save(path,rows):
    with path.open("w",newline="") as stream:
        w=csv.DictWriter(stream,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)

def audit(folder,out,label,mode,rep,variant,metrics,export):
    file=out/(label+"-attempts.csv.gz")
    if export:
        log=(folder/"app.log").read_text();assert "INDEPENDENT_OK,16384" in log
        attempts=[tuple(map(int,m.split(","))) for m in re.findall(r"^ATTEMPT,([0-9,]+)$",log,re.M)]
        with gzip.open(file,"wt",newline="") as stream:
            w=csv.writer(stream,lineterminator="\n")
            w.writerow("request received begin commit lock acquired sync flushed bytes end sequence".split())
            w.writerows(attempts)
    with gzip.open(file,"rt",newline="") as stream:
        r=csv.reader(stream);next(r);attempts=[tuple(map(int,row)) for row in r]
    assert all(a[1]<=a[2]<=a[3]<=a[4]<=a[5]<=a[9] for a in attempts)
    rows=summarize_attempts(attempts,16384)
    top=rows[:164];worst=top[0]; p99=rows[163]["total_ms"]
    # Nearest-rank p99 for 16,384 samples is the 164th largest.
    throughput=len(rows)*1e9/(max(a[9] for a in attempts)-min(a[1] for a in attempts))
    for k,v in (("max_ms",worst["total_ms"]),("p99_ms",p99),("updates_s",throughput)):
        assert math.isclose(float(metrics[k]),v,rel_tol=1e-9),(label,k,metrics[k],v)
    if "worst_passes" in metrics:
        assert int(metrics["worst_passes"])==worst["overtakes"]
    return dict(series=label.split("-")[0],mode=mode,rep=rep,variant=variant,n=len(rows),
                p99_ms=p99,max_ms=worst["total_ms"],updates_s=throughput,
                all_passed_requests=sum(r["overtakes"]>0 for r in rows),
                total_passes=sum(r["overtakes"] for r in rows),late16=sum(r["after16"] for r in rows),
                max_passes=max(r["overtakes"] for r in rows),top_passed=sum(r["overtakes"]>0 for r in top),
                top_median_passes=median(r["overtakes"] for r in top),worst_id=worst["request"],
                worst_passes=worst["overtakes"],worst_distinct=worst["distinct"],worst_after16=worst["after16"],
                worst_pass_sync_ms=worst["passing_sync_ms"],worst_peer_sync_ms=worst["all_sync_ms"],
                worst_peer_syncs=worst["sync_count"],worst_longest_sync_ms=worst["longest_sync_ms"],
                worst_lock_ms=worst["lock_ms"])

def main():
    p=argparse.ArgumentParser();p.add_argument("out",type=Path)
    p.add_argument("--old",type=Path);p.add_argument("--fresh",type=Path);args=p.parse_args()
    if args.old or args.fresh:
        bench=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
        prev=bench.parent/"docs/benchmarks/production-range-writeback-2026-11-08"
        for published,destination in (("trials.csv","prior-trials.csv"),("tail-requests.csv","prior-tails.csv")):
            if not (args.out/destination).exists():shutil.copyfile(prev/published,args.out/destination)
    old=load(args.out/"prior-trials.csv");new=load(args.out/"trials.csv")
    assert len(old)==12 and len(new)==9
    pubs=load(args.out/"publications.csv")
    assert len(pubs)==567
    for row in new:
        group=[x for x in pubs if (x["mode"],x["rep"])==(row["mode"],row["rep"])]
        assert sorted(int(x["publication"]) for x in group)==list(range(1,64))
        assert sum(int(x["bio_B"]) for x in group)==int(row["submitted_B"])
    all_rows=[]
    for series,source,data in (("previous",args.old,old),("new",args.fresh,new)):
        for row in data:
            mode,rep,v=(row[k] for k in ("mode","rep","variant"))
            label=f"{series}-{mode}-{v}-{rep}"
            if series=="previous" and args.fresh and not args.old:
                target=args.out/(label+"-attempts.csv.gz")
                if not target.exists():
                    published=bench.parent/"docs/benchmarks/aged-writer-2026-11-08"/target.name
                    shutil.copyfile(published,target)
            folder=None if not source else (source/v if series=="previous" else source)/f"{rep}-{mode}-trace"
            all_rows.append(audit(folder,args.out,label,mode,rep,v,row,bool(source)))
    if (args.out/"prior-tails.csv").exists():
        previous={(r["mode"],r["variant"],r["rep"]):r for r in load(args.out/"prior-tails.csv") if r["label"]=="worst"}
        for a in (r for r in all_rows if r["series"]=="previous"):
            b=previous[a["mode"],a["variant"],a["rep"]]
            for key,old_key in (("worst_id","id"),("worst_lock_ms","writer_wait_ms"),
                                ("worst_peer_sync_ms","writer_wait_peer_sync_ms"),("worst_peer_syncs","peer_sync_intervals"),
                                ("worst_longest_sync_ms","longest_peer_sync_overlap_ms")):
                assert math.isclose(a[key],float(b[old_key]),abs_tol=1e-8)
    save(args.out/"overtaking.csv",all_rows)
    print("Reconciled 344,064 requests in 21 trials and 567 new publications.")

if __name__=="__main__":main()
