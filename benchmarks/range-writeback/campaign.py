"""Compare asynchronous 8 MiB range writeback with the existing final fsync."""
import argparse
import csv
import gzip
import json
import os
import sys
from collections import Counter, defaultdict, deque
from pathlib import Path
from statistics import median

BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"ext4-checkpoint-bytes"))
from run import one
from join import run as join_trace
from identity import analyze,records
from report_independent import save

MIB=2**20
WINDOW=10_000_000
IMAGE="catdb-range-writeback"

def burst(events):
    samples=deque();amount=peak=0;bins=Counter()
    for at,size in events:
        while samples and samples[0][0]<=at-WINDOW:
            amount-=samples.popleft()[1]
        samples.append((at,size));amount+=size;peak=max(peak,amount)
        bins[at//WINDOW]+=size
    return peak,sum(b for b in bins.values() if b>=32*MIB)

def measure(folder,label,mode,rep,pilot,results):
    ops=768 if pilot else 16384
    log=(folder/"app.log").read_text()
    trial,_,_=analyze(log,mode,rep,ops,256)
    assert trial["checkpoints"]==ops//256-1 and not trial["coalesced"]
    by=defaultdict(list)
    pubs,epochs,stats=join_trace(folder,lambda i,t,b:by[i].append((t,b)))
    peak=[];bulk=0;phases=records(log,"PUBLICATION")
    for i,p in enumerate(pubs):
        hi,large=burst(by[i]);p.update(peak10_B=hi,burst32_B=large,
                                    publish_ms=(phases[i][11]-phases[i][3])/1e6)
        assert sum(b for _,b in by[i])==p["bio_B"]
        assert abs(p["bio_B"]-p["app_B"])<max(128*1024,.02*p["app_B"])
        peak.append(hi/MIB);bulk+=large
    byte_sum=sum(p["bio_B"] for p in pubs)
    trial.update(variant=label,logical_B=sum(p["app_B"] for p in pubs),submitted_B=byte_sum,
                 peak10_MiB=median(peak),bulk_pct=100*bulk/byte_sum,
                 publish_ms=median(p["publish_ms"] for p in pubs),
                 from_first_to_last_publication_s=(phases[-1][11]-phases[0][1])/1e9,
                 journal_med_ms=median(r["duration_us"] for r in epochs)/1000,
                 journal_p99_ms=sorted(r["duration_us"] for r in epochs)[(99*len(epochs)+99)//100-1]/1000,
                 attributed=stats["wal_resolved"],unattributed=ops-stats["wal_resolved"])
    for p in pubs:p.update(variant=label,mode=mode,rep=rep)
    results.extend(pubs)
    print(f"{mode} {label}: peak {trial['peak10_MiB']:.1f} MiB, burst bytes {trial['bulk_pct']:.1f}%, updates/s {trial['updates_s']:.1f}",flush=True)
    return trial

def campaign(root,out,pilot):
    root.mkdir();out.mkdir(parents=True,exist_ok=True)
    trials=[];pubs=[]
    for rep in range(1,2 if pilot else 4):
        order=[("standard","baseline"),("standard","range8"),("fifo","range8"),("fifo","baseline")]
        if rep==2:order=list(reversed(order))
        if rep==3:order=order[2:]+order[:2]
        for mode,label in order:
            dest=root/label;dest.mkdir(exist_ok=True)
            one(dest,rep,mode,True,False,calibration=pilot,image=IMAGE,range_writeback=label=="range8")
            trials.append(measure(dest/f"{rep}-{mode}-trace",label,mode,rep,pilot,pubs))
            prefix="pilot-" if pilot else ""
            save(out/(prefix+"trials.csv"),trials);save(out/(prefix+"publications.csv"),pubs)
    return trials

def passes(trials):
    for mode in ("standard","fifo"):
        base,changed=[next(r for r in trials if r["mode"]==mode and r["variant"]==v) for v in ("baseline","range8")]
        if not (changed["peak10_MiB"]<=.8*base["peak10_MiB"] and changed["bulk_pct"]<base["bulk_pct"]
                and abs(changed["submitted_B"]/base["submitted_B"]-1)<=.02):
            return False
    return True

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("root",type=Path);p.add_argument("out",type=Path);a=p.parse_args()
    a.root.mkdir(parents=True,exist_ok=False)
    proceed=passes(campaign(a.root/"pilot",a.out,True))
    print("pilot_gate",proceed,flush=True)
    if proceed:campaign(a.root/"full",a.out,False)
