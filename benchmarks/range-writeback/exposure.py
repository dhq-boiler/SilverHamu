"""Export one replayable row per WAL-targeted epoch and report exposure changes."""
import argparse
import csv
import gzip
import json
from collections import defaultdict
from pathlib import Path
from statistics import median
from campaign import MIB,join_trace,save

def summarize(folder,mode,variant,rep,out):
    pubs,epochs,stats=join_trace(folder)
    name=f"{rep}-{mode}-{variant}"
    buckets=defaultdict(list)
    with gzip.open(out/(name+"-epochs.csv.gz"),"wt",newline="") as f:
        writer=csv.DictWriter(f,epochs[0].keys(),lineterminator="\n")
        writer.writeheader();writer.writerows(epochs)
    for r in epochs:
        for window in ("prior","during","first5"):
            if window=="first5" and r["remaining_at_5ms_us"]=="":continue
            same,other,unknown=[r[f"{window}_{key}_B"] for key in ("same","other","unknown")]
            if same or unknown:label="same_or_ambiguous"
            elif other==0:label="zero"
            elif other>=32*MIB:label="different_32_MiB_or_more"
            else:label="different_below_32_MiB"
            key="remaining_at_5ms_us" if window=="first5" else "duration_us"
            buckets[window,label].append(r[key]/1000)
    meta=json.loads((folder/"meta.json").read_text())
    quality=dict(mode=mode,variant=variant,rep=rep,kernel=meta["kernel"],device=meta["device"],
                 resolved=stats["wal_resolved"],wal=stats["wal"],probe_misses=max(int(l.split()[-1]) for l in meta["probe_profile"]),
                 ring_loss=0,cleaned=meta["cleaned"])
    return [dict(mode=mode,variant=variant,rep=rep,window=w,exposure=k,n=len(v),median_ms=median(v))
            for (w,k),v in sorted(buckets.items())],quality

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("full",type=Path);p.add_argument("out",type=Path);args=p.parse_args()
    groups=[];quality=[]
    for variant in ("baseline","range8"):
        for folder in sorted((args.full/variant).glob("*-trace")):
            rep,mode,_=folder.name.split("-")
            rows,trace=summarize(folder,mode,variant,int(rep),args.out)
            groups.extend(rows);quality.append(trace)
    assert len(quality)==12
    save(args.out/"journal-groups.csv",groups);save(args.out/"trace-quality.csv",quality)
