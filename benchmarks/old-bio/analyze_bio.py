"""Pair checkpoint BIOs by live pointer lifetime. Export only numeric workload observations."""
from bisect import bisect_left, bisect_right
from collections import defaultdict
import csv
import gzip
import json
import os
from pathlib import Path
import re
import sys

BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"scaled-gate"))
from report_independent import records

STAMP=re.compile(r"\s(\d+)\.(\d+): (\w+): (.*)")
FIELDS=re.compile(r"bytes=(\d+) ino=(\d+) dev=(\d+) tid=\d+ token=(0x[0-9a-fA-F]+)")
WINDOW=10_000_000

def classify(start,end,active,starts):
    prior=bisect_left(starts,start)-bisect_left(starts,start-WINDOW)
    during=bisect_left(starts,end)-bisect_left(starts,start)
    old=[(s,e,b) for s,e,b in active if s<=start-WINDOW and e>start]
    return prior,during,len(old),sum(b for _,_,b in old)

def audit(folder,ops,cap,*,bound=False):
    meta=json.loads((folder/"meta.json").read_text())
    assert meta["cleaned"] and len(meta["probe_profile"])==4
    assert all(int(x.split()[-1])==0 for x in meta["probe_profile"] if not (bound and x.strip().startswith("completed_")))
    assert all(int(n)==0 for v in meta["stats"].values() for n in re.findall(r"^(?:overrun|commit overrun|dropped events):\s*(\d+)",v,re.M))
    log=(folder/"app.log").read_text()
    assert records(log,"INDEPENDENT_OK")==[(ops,)]
    clocks=records(log,"OS_CLOCK")
    assert len(clocks)==2 and all(w==hi-lo<2000 for w,lo,hi in clocks)
    offset=(max(x[1] for x in clocks)+min(x[2] for x in clocks))//2
    pubs=records(log,"PUBLICATION");assert [p[0] for p in pubs]==list(range(1,ops//cap))
    syncs=[]; events=[]
    with gzip.open(folder/"trace.gz","rt") as f:
        for line in f:
            m=STAMP.search(line)
            if not m:continue
            sec,frac,kind,body=m.groups()
            t=int(sec)*10**9+int(frac.ljust(9,"0"))-offset
            if kind=="ext4_sync_file_enter" and ("cw"+meta["tag"]+"4-") in line:
                syncs.append((t,int(re.search(r" ino (\d+)",body)[1])))
            if kind=="bio_"+meta["tag"]:
                b,ino,dev,ptr=FIELDS.search(body).groups()
                events.append((t,True,int(ino),int(ptr,16),int(b)))
            elif kind=="completed_"+meta["tag"]:
                ptr=int(re.search(r"token=(0x[0-9a-fA-F]+)",body)[1],16)
                events.append((t,False,None,ptr,0))
    inodes=[]
    for p in pubs:
        match=[ino for t,ino in syncs if p[4]-2000<=t<=p[5]+2000]
        assert len(match)==1,("snapshot inode",p[0],match)
        inodes.extend(match)
    beg=[p[1] for p in pubs]; live={}; paired=[]; seen=set(); reused=0; counts=defaultdict(int)
    capped=0
    for t,submit,ino,ptr,b in events:
        if submit:
            if ptr in live:
                assert bound,"new submission before target completion"
                s,_,size=live.pop(ptr); paired.append((s,t,size));capped+=1
            i=bisect_right(beg,t)-1
            if i>=0 and t<pubs[i][8] and ino==inodes[i]:
                assert b>0
                live[ptr]=(t,ino,b);reused+=ptr in seen;seen.add(ptr);counts[i]+=1
        elif ptr in live:
            s,expected,b=live.pop(ptr);assert t>=s
            paired.append((s,t,b))
    if bound:
        horizon=max(a[9] for a in records(log,"ATTEMPT"))+1
        paired.extend((s,max(s,horizon),size) for s,_,size in live.values())
        capped+=len(live)
    assert (bound or not live) and len(counts)==len(pubs)
    paired.sort(); starts=[x[0] for x in paired]
    active=[];i=0;rows=[]
    for a in sorted((a for a in records(log,"ATTEMPT") if a[-1]),key=lambda a:a[6]):
        s,end=a[6:8]
        while i<len(paired) and paired[i][0]<=s:
            active.append(paired[i]);i+=1
        active=[x for x in active if x[1]>s]
        prior,during,n,b=classify(s,end,active,starts)
        last=max((e for t,e,_ in active if t<=s-WINDOW),default=s)
        rows.append((a[0],end-s,prior,during,n,b,s,min(end,last)-s))
    assert sorted(r[0] for r in rows)==list(range(ops))
    prefix="upper-" if bound else ""
    with gzip.open(folder/(prefix+"calls.csv.gz"),"wt",newline="") as f:
        w=csv.writer(f);w.writerow(("request","sync_ns","prior_n","during_n","old_pending_n","old_pending_B","sync_start_ns","old_overlap_ns"));w.writerows(rows)
    with gzip.open(folder/(prefix+"lifetimes.csv.gz"),"wt",newline="") as f:
        w=csv.writer(f);w.writerow(("submit_ns","callback_ns","bytes"));w.writerows(paired)
    quiet=[r for r in rows if not (r[2] or r[3])]
    result=dict(paired=len(paired),bytes=sum(x[2] for x in paired),reused=reused,quiet=len(quiet),quiet_old=sum(r[4]>0 for r in quiet),ops=ops,
                long_bios=sum(e-s>=WINDOW for s,e,b in paired),max_life_ms=max(e-s for s,e,b in paired)/1e6)
    result.update(upper_bound=bound,capped_at_reuse_or_horizon=capped)
    (folder/(prefix+"pairing.json")).write_text(json.dumps(result))
    print(folder.name,result,flush=True)
    return result
