"""Join checkpoint inode lifetimes and ext4 data submissions to WAL commit epochs."""
import bisect
import gzip
import json
import os
from pathlib import Path
import re
import sys
from collections import Counter

BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"journal-identity"))
from identity import LINE, identity, records

WINDOW=10_000_000

def byte_bins(events, times, lo, hi, tx):
    same=other=unknown=0
    for _,size,tid,fsync_tid in events[bisect.bisect_left(times,lo):bisect.bisect_left(times,hi)]:
        if not tid or not fsync_tid: unknown+=size
        elif tid==tx and fsync_tid==tx: same+=size
        elif tid!=tx and fsync_tid!=tx: other+=size
        else: unknown+=size
    return same,other,unknown

def run(folder):
    meta=json.loads((folder/"meta.json").read_text())
    log=(folder/"app.log").read_text()
    stats,calls=identity(folder,("bio",))
    anchors=records(log,"OS_CLOCK")
    origin=(max(a[1] for a in anchors)+min(a[2] for a in anchors))//2
    cp=records(log,"PUBLICATION")
    ranges=[(origin+c[1],origin+c[8]) for c in cp]
    beginnings=[lo for lo,_ in ranges]
    syncs=[d for d in calls if d["phase"]=="snapshot"]
    inodes=[];bio=[];writes=[];sync_events=[]
    with gzip.open(folder/"trace.gz","rt") as stream:
        for line in stream:
            m=LINE.match(line)
            if not m:continue
            comm,_,_,sec,frac,body=m.groups()
            t=int(sec)*10**9+int(frac.ljust(9,"0"))
            if body.startswith("bio_"+meta["tag"]+":"):
                size,ino,dev,tid=map(int,re.search(r"bytes=(\d+) ino=(\d+) dev=(\d+) tid=(\d+)",body).groups())
                bio.append((t,size,tid,ino))
            elif comm.strip()=="cw"+meta["tag"]+"4":
                if body.startswith("ext4_sync_file_enter:"):
                    sync_events.append((t,int(re.search(r" ino (\d+)",body)[1])))
                elif re.match(r"sys_(?:p?write(?:64|v)?) ->",body):
                    size=int(body.split("->")[1].strip(),0)
                    assert 0<=size<2**31
                    writes.append((t,size))
    for sync in syncs:
        match=[ino for t,ino in sync_events if sync["begin"]<=t<=sync["end"]]
        assert len(match)==1
        inodes.append(match[0])
    assert len(inodes)==len(cp)
    tails=[None]*len(cp)
    for d in calls:
        if d["phase"]=="tail":
            i=bisect.bisect_right(beginnings,d["begin"])-1
            assert tails[i] is None
            tails[i]=d["tx"]
    events=[];volumes=Counter()
    for t,size,tid,ino in bio:
        i=bisect.bisect_right(beginnings,t)-1
        if i>=0 and t<ranges[i][1] and ino==inodes[i]:
            target=syncs[i]["tx"] if t<=syncs[i]["end"] else tails[i]
            events.append((t,size,tid,target));volumes[i]+=size
    times=[x[0] for x in events]
    wt=[x[0] for x in writes]
    prefix=[0]
    for _,size in writes:prefix.append(prefix[-1]+size)
    def logical(lo,hi):
        return prefix[bisect.bisect_left(wt,hi)]-prefix[bisect.bisect_left(wt,lo)]
    pubs=[dict(publication=c[0],app_B=c[-1],write_returns_B=logical(lo,hi),bio_B=volumes[i]) for i,(c,(lo,hi)) in enumerate(zip(cp,ranges))]
    # Only successful writes within a publication can enter a WAL window.
    writes=[(t,size) for t,size in writes if (i:=bisect.bisect_right(beginnings,t)-1)>=0 and t<ranges[i][1]]
    wt=[x[0] for x in writes];prefix=[0]
    for _,size in writes:prefix.append(prefix[-1]+size)
    epochs={}
    for call in calls:
        if call["phase"]!="wal" or call["status"]!="blocked_at_end":continue
        start,end,tx=call["commit_start"],call["completed"],call["tx"]
        if (tx,end) in epochs:
            epochs[tx,end]["writers"]+=1
            continue
        idx=bisect.bisect_right(beginnings,start)-1
        phase="outside"
        if idx>=0:
            c=cp[idx]
            for j,k,label in ((1,2,"prefix"),(2,3,"await_trigger"),(3,4,"encode"),(4,5,"snapshot_fsync"),(5,6,"await_lock"),(6,10,"cutover"),(10,11,"finish")):
                if origin+c[j]<=start<origin+c[k]:
                    phase=label
                    break
        row=dict(tx=tx,start_ns=start,duration_us=(end-start)/1000,writers=1,
                 publication=idx+1 if phase!="outside" else 0,phase=phase)
        for label,lo,hi in (("prior",start-WINDOW,start),("during",start,end)):
            for key,size in zip(("same","other","unknown"),byte_bins(events,times,lo,hi,tx)):
                row[label+"_"+key+"_B"]=size
            subset=events[bisect.bisect_left(times,lo):bisect.bisect_left(times,hi)]
            row[label+"_inode_same_later_other_B"]=sum(b for _,b,tid,later in subset if tid==tx and later and later!=tx)
            row[label+"_inode_other_later_same_B"]=sum(b for _,b,tid,later in subset if tid and tid!=tx and later==tx)
            row[label+"_target_missing_B"]=sum(b for _,b,tid,later in subset if not tid or not later)
            assert sum(row[label+"_"+k+"_B"] for k in ("inode_same_later_other","inode_other_later_same","target_missing"))==row[label+"_unknown_B"]
            row[label+"_app_B"]=logical(lo,hi)
        eligible=end-start>=5_000_000
        row["remaining_at_5ms_us"]=(end-start-5_000_000)/1000 if eligible else ""
        bins=byte_bins(events,times,start,start+5_000_000,tx) if eligible else ("","","")
        for key,size in zip(("same","other","unknown"),bins):
            row["first5_"+key+"_B"]=size
        epochs[tx,end]=row
    assert sum(r["writers"] for r in epochs.values())==stats["wal_resolved"]
    assert all(r["bio_B"]>0 for r in pubs)
    assert all(r["app_B"]==r["write_returns_B"] for r in pubs), "logical byte mismatch"
    return pubs,list(epochs.values()),stats

if __name__=="__main__":
    for path in sys.argv[1:]:
        p,rows,stats=run(Path(path))
        print(path,p[0],sum(r["bio_B"] for r in p),sum(r["write_returns_B"] for r in p),sum(r["app_B"] for r in p),len(rows),sum(r["prior_other_B"]>0 for r in rows))
