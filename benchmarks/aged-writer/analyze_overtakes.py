"""Reconstruct later-arriving writer admissions and fsync overlap from ATTEMPT records."""
import bisect
import csv
import gzip
import math
import re
from collections import defaultdict
from pathlib import Path
from statistics import median
import sys

def analyze(folder):
    log=(folder/"app.log").read_text()
    assert "INDEPENDENT_OK,16384" in log
    attempts=[tuple(map(int,m.split(","))) for m in re.findall(r"^ATTEMPT,([0-9,]+)$",log,re.M)]
    return summarize_attempts(attempts,16384)

def summarize_attempts(attempts,expected):
    by_acq=sorted(attempts,key=lambda r:r[5])
    times=[r[5] for r in by_acq]
    synced=[r for r in by_acq if r[-1]]
    assert len(synced)==expected and all(r[7]<=s[6] for r,s in zip(synced,synced[1:]))
    sync_ends=[r[7] for r in synced];sync_starts=[r[6] for r in synced]
    groups=defaultdict(list)
    for row in attempts:groups[row[0]].append(row)
    assert set(groups)==set(range(expected))
    assert all(rows[-1][-1] and all(not r[-1] for r in rows[:-1]) for rows in groups.values())
    output=[]
    for key,tries in groups.items():
        passes=[];overlaps=[];wait=0;late_passes=0;last_barge_ns=0
        for r in tries:
            lo,hi=r[4:6];wait+=hi-lo
            for other in by_acq[bisect.bisect_right(times,lo):bisect.bisect_left(times,hi)]:
                if other[0]!=key and lo<other[4]:
                    passes.append(other)
                    late_passes+=int(other[4]-lo>=16_000_000)
                    last_barge_ns=max(last_barge_ns,other[4]-lo)
            for other in synced[bisect.bisect_right(sync_ends,lo):bisect.bisect_left(sync_starts,hi)]:
                if other[0]!=key:
                    overlaps.append(min(hi,other[7])-max(lo,other[6]))
        assert sum(overlaps)<=wait
        output.append(dict(request=key,total_ms=(tries[-1][9]-tries[0][1])/1e6,lock_ms=wait/1e6,
                           overtakes=len(passes),distinct=len({s[0] for s in passes}),
                           after16=late_passes,last_pass_start_ms=last_barge_ns/1e6,passing_commits=sum(s[-1]!=0 for s in passes),
                           passing_sync_ms=sum(s[7]-s[6] for s in passes)/1e6,
                           all_sync_ms=sum(overlaps)/1e6,longest_sync_ms=max(overlaps,default=0)/1e6,
                           sync_count=len(overlaps)))
    return sorted(output,key=lambda r:(r["total_ms"],r["request"]),reverse=True)

if __name__=="__main__":
    root=Path(sys.argv[1]);dest=Path(sys.argv[2]);dest.mkdir(exist_ok=True)
    summaries=[]
    for folder in sorted(root.glob("*/*-trace")):
        rep,mode,_=folder.name.split("-");variant=folder.parent.name
        rows=analyze(folder)
        top=rows[:math.ceil(len(rows)/100)]
        with gzip.open(dest/f"{variant}-{mode}-{rep}.csv.gz","wt",newline="") as f:
            w=csv.DictWriter(f,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)
        summaries.append(dict(variant=variant,mode=mode,rep=rep,top=len(top),
            top_with_passes=sum(r["overtakes"]>0 for r in top),top_median_passes=median(r["overtakes"] for r in top),
            **rows[0]))
    with (dest/"summary.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,summaries[0],lineterminator="\n");w.writeheader();w.writerows(summaries)
    assert len(summaries)==12
