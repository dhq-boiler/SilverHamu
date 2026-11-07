"""Compare standard, strict FIFO, and 16 ms handoff with identical tagged workers."""
import argparse
import csv
import gzip
import json
import os
from pathlib import Path
import sys
from statistics import median
import subprocess

from analyze_overtakes import analyze as overtakes
BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"range-writeback"))
from campaign import one, measure, save

def run(raw,out,image):
    raw.mkdir(parents=True,exist_ok=False);out.mkdir(parents=True,exist_ok=False)
    trials=[];pubs=[]
    (out/"images.json").write_text(json.dumps({n:subprocess.check_output(["docker","image","inspect","--format","{{.Id}}",n],text=True).strip() for n in (image,"catdb-fifo-products")},indent=2))
    orders=[("standard","fifo","aged"),("fifo","aged","standard"),("aged","standard","fifo")]
    for rep,order in enumerate(orders,1):
        for mode in order:
            folder=raw/f"{rep}-{mode}-trace"
            for retry in range(3):
                one(raw,rep,mode,True,False,image=image,range_writeback=True)
                m=json.loads((folder/"meta.json").read_text())
                import re
                lost=sum(int(v) for s in m["stats"].values() for v in re.findall(r"^(?:overrun|commit overrun|dropped events):\s*(\d+)",s,re.M))
                if not lost:break
                excluded=raw/f"{rep}-{mode}-lost-{retry+1}"
                assert raw.resolve() in folder.resolve().parents and not excluded.exists()
                folder.rename(excluded)
                print("Excluded trace",excluded.name,lost,flush=True)
            else:raise RuntimeError("repeated kernel loss")
            trial=measure(folder,"range8",mode,rep,False,pubs)
            rows=overtakes(folder);top=rows[:164];worst=rows[0]
            trial.update(top_1pct_with_passes=sum(r["overtakes"]>0 for r in top),
                         top_1pct_median_passes=median(r["overtakes"] for r in top),
                         max_passes=max(r["overtakes"] for r in rows),worst_request=worst["request"],
                         worst_passes=worst["overtakes"],worst_lock_ms=worst["lock_ms"],
                         worst_peer_sync_ms=worst["all_sync_ms"],worst_longest_sync_ms=worst["longest_sync_ms"])
            trials.append(trial)
            with gzip.open(out/(folder.name+".csv.gz"),"wt",newline="") as f:
                w=csv.DictWriter(f,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)
            save(out/"trials.csv",trials);save(out/"publications.csv",pubs)
            print("MEASURED",rep,mode,trial["p99_ms"],trial["max_ms"],trial["updates_s"],worst["overtakes"],flush=True)

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("raw",type=Path);p.add_argument("out",type=Path)
    p.add_argument("--image",default="catdb-aged-writer");a=p.parse_args()
    run(a.raw,a.out,a.image)
