"""Recompute exposure by walking from each BIO to covered sync starts."""
from bisect import bisect_left
import csv
import gzip
from pathlib import Path
import sys

WINDOW=10_000_000

def load(path):
    with gzip.open(path,"rt") as f:
        return [{k:int(v) for k,v in r.items()} for r in csv.DictReader(f)]

def replay(calls,bios,shift=0):
    starts=[r["sync_start_ns"]+shift for r in calls]
    assert sorted(starts)==starts
    submitted=sorted(b["submit_ns"] for b in bios)
    count=[0]*len(calls);volume=[0]*len(calls);overlap=[0]*len(calls)
    for b in bios:
        left=bisect_left(starts,b["submit_ns"]+WINDOW)
        right=bisect_left(starts,b["callback_ns"])
        for i in range(left,right):
            count[i]+=1;volume[i]+=b["bytes"]
            overlap[i]=max(overlap[i],min(calls[i]["sync_ns"],b["callback_ns"]-starts[i]))
    quiet=pending=0
    for i,(s,r) in enumerate(zip(starts,calls)):
        left=bisect_left(submitted,s-WINDOW);middle=bisect_left(submitted,s)
        end=bisect_left(submitted,s+r["sync_ns"])
        if not shift:
            assert (middle-left,end-middle,count[i],volume[i])==(r["prior_n"],r["during_n"],r["old_pending_n"],r["old_pending_B"])
            assert overlap[i]==r["old_overlap_ns"]
        if end==left:
            quiet+=1; pending+=count[i]>0
    return quiet,pending

if __name__=="__main__":
    for d in map(Path,sys.argv[1:]):
        calls=load(d/"calls.csv.gz");bios=load(d/"lifetimes.csv.gz")
        print(d.name,[replay(calls,bios,shift) for shift in (-2000,0,2000)])
