"""Replay application markers and quantify end-to-end latency and active lock time."""
import argparse
from bisect import bisect_left, bisect_right
import csv
import gzip
import json
import math
import shutil
from collections import defaultdict
from pathlib import Path
from statistics import median
from campaign import CASES, BENCH, accounting, percentile, records, responses, save

def load(path):
    with path.open(newline="") as f:return list(csv.DictReader(f))

def isolated_acquisitions(holds):
    ordered=sorted(holds,key=lambda h:h[2])
    starts=sorted(h[1] for h in ordered)
    acquired=[h[2] for h in ordered]
    observed=[]
    previous_release=-1
    for lane,start,acq,before,returned in ordered:
        finished_before=previous_release<start
        prior_waiters=bisect_left(starts,start)-bisect_left(acquired,start)
        starts_in_call=bisect_right(starts,acq)-bisect_left(starts,start)
        if lane!=4 and finished_before and prior_waiters==0 and starts_in_call==1:
            observed.append(acq-start)
        previous_release=max(previous_release,returned)
    return observed

def check_case(log,case,mode,rep,trial,request_file):
    lanes,rate,ops=CASES[case]
    origin,l,r=records(log,"LOAD_SCHEDULE")[0]
    assert (l,r)==(lanes,rate) and records(log,"INDEPENDENT_OK")==[(ops,)]
    tries=records(log,"ATTEMPT");holds=records(log,"HOLD");pubs=records(log,"PUBLICATION")
    assert [c[0] for c in pubs]==list(range(1,ops//256))
    arrivals=responses(tries,origin,rate,ops)
    with gzip.open(request_file,"rt",newline="") as f:
        reader=csv.reader(f);next(reader);assert arrivals==[tuple(map(int,row)) for row in reader]
    due=[a[3]-a[1] for a in arrivals];sent=[a[3]-a[2] for a in arrivals];late=[a[2]-a[1] for a in arrivals]
    ac=accounting(log,ops,lanes,allow_idle=bool(rate))
    for key,actual in {**ac,"scheduled_p99_ms":percentile(due),"sent_p99_ms":percentile(sent),
                       "send_lag_p99_ms":percentile(late),"max_scheduled_ms":max(due)/1e6,
                       "max_sent_ms":max(sent)/1e6,"updates_s":ops/ac["response_s"]}.items():
        assert math.isclose(float(trial[key]),actual,abs_tol=1e-9,rel_tol=1e-10),(key,trial[key],actual)
    waits=[a-s for lane,s,a,b,e in holds if lane!=4]
    syncs=[t[7]-t[6] for t in tries if t[-1]]
    overlap_count=0
    for req,d,s,e,_,_ in arrivals:
        if any(c[3]<=e and s<c[11] for c in pubs):overlap_count+=1
    by_id=defaultdict(list)
    for t in tries:by_id[t[0]].append(t)
    backlog=[]
    for index,d,s,e,*_ in arrivals:
        prior_end=arrivals[index-lanes][3] if index>=lanes else origin
        assert prior_end<=s
        debt=max(0,prior_end-d) if rate else 0
        assert debt<=s-d
        backlog.append(debt)
    ordered=sorted(range(ops),key=lambda i:(due[i],i))
    selected=[]
    for label,i in (("p99",ordered[math.ceil(.99*ops)-1]),("worst",ordered[-1])):
        spans=by_id[i]
        wait=sum(t[5]-t[4] for t in spans)
        sync=sum(t[7]-t[6] for t in spans if t[-1])
        other=sent[i]-wait-sync
        assert other>=0
        selected.append(dict(case=case,mode=mode,rep=rep,label=label,request=i,
                        scheduled_ms=due[i]/1e6,dispatch_lag_ms=late[i]/1e6,lane_busy_ms=backlog[i]/1e6,
                        writer_wait_ms=wait/1e6,wal_ms=sync/1e6,other_service_ms=other/1e6))
    clean=isolated_acquisitions(holds)
    return selected,dict(isolated_n=len(clean),isolated_p99_us=percentile(clean)*1000 if clean else "",
                         isolated_median_us=median(clean)/1000 if clean else "",case=case,mode=mode,rep=rep,backlogged=sum(b>0 for b in backlog),max_backlog_ms=max(backlog)/1e6,lock_median_us=median(waits)/1000,
                lock_p99_us=percentile(waits)*1000,wal_p99_ms=percentile(syncs),
                wal_mean_ms=sum(syncs)/ops/1e6,scheduled_rate=ops*1e9/(max(a[3] for a in arrivals)-min(a[1] for a in arrivals)),
                max_send_lag_ms=max(late)/1e6,late_by_global_period=sum(v>=10**9//rate for v in late) if rate else "",
                overlaps_publication=overlap_count,publications_after_last=sum(c[11]>max(a[3] for a in arrivals) for c in pubs))

def old_study(raw,out):
    previous=[]
    for m in ("standard","fifo","aged"):
        for rep in range(1,4):
            name=f"{rep}-{m}-trace"
            path=out/f"previous-{name}-markers.txt.gz"
            if raw is not None:
                log=(raw/name/"app.log").read_text()
                with gzip.open(path,"wt") as f:
                    for line in log.splitlines():
                        if line.startswith(("ATTEMPT,","PUBLICATION,","HOLD,","INDEPENDENT_OK,")):f.write(line+"\n")
            with gzip.open(path,"rt") as f:log=f.read()
            assert records(log,"INDEPENDENT_OK")==[(16384,)]
            a=accounting(log,16384,4);pubs=records(log,"PUBLICATION")
            last=max(t[9] for t in records(log,"ATTEMPT"))
            a.update(mode=m,rep=rep,post_response_pubs=sum(c[11]>last for c in pubs),updates_s=16384/a["response_s"])
            previous.append(a)
    reference=BENCH.parent/"docs/benchmarks/aged-writer-2026-11-08/trials.csv"
    originals={(r["mode"],int(r["rep"])):r for r in load(reference)}
    for r in previous:
        ref=originals[r["mode"],r["rep"]]
        assert math.isclose(r["updates_s"],float(ref["updates_s"]),rel_tol=1e-10)
        assert math.isclose(16384/r["drain_s"],float(ref["drained_s"]),rel_tol=1e-10)
    save(out/"previous.csv",previous)

def main():
    p=argparse.ArgumentParser();p.add_argument("out",type=Path);p.add_argument("--previous-raw",type=Path);a=p.parse_args()
    out=a.out
    published=BENCH.parent/"docs/benchmarks/aged-load-2026-11-08"
    if a.previous_raw is None:
        for source in published.glob("previous-*"):
            if source.is_file() and not (out/source.name).exists():shutil.copyfile(source,out/source.name)
    old_study(a.previous_raw,out)
    if (out/"previous-kernel.csv").exists():
        for r in load(out/"previous-kernel.csv"):
            path=out/f"previous-{r['mode']}-{r['rep']}-wal.csv.gz"
            with gzip.open(path,"rt",newline="") as f: calls=list(csv.DictReader(f))
            assert sorted(int(c["request"]) for c in calls)==list(range(16384))
            for field,k in (("sync_ns","syscall_s"),("journal_ns","journal_sleep_s"),("before_start_ns","before_start_s"),
                            ("during_commit_ns","during_commit_s"),("after_end_ns","after_end_s")):
                assert math.isclose(sum(int(c[field]) for c in calls)/1e9,float(r[k]),abs_tol=1e-9)
    trials=load(out/"trials.csv");quality=load(out/"quality.csv")
    expected={(case,mode,rep) for case in CASES for mode in ("standard","fifo","aged") for rep in (1,2,3)}
    assert len(trials)==len(quality)==len(expected)==27
    assert {(r["case"],r["mode"],int(r["rep"])) for r in trials}==expected
    assert all(int(r["ring_loss"])==int(r["probe_misses"])==0 for r in quality)
    diagnostics=[];tails=[]
    for r in trials:
        name=f"{r['case']}-{r['mode']}-{r['rep']}"
        with gzip.open(out/(name+"-markers.txt.gz"),"rt") as f:log=f.read()
        points,summary=check_case(log,r["case"],r["mode"],int(r["rep"]),r,out/(name+"-requests.csv.gz"))
        diagnostics.append(summary);tails.extend(points)
    save(out/"diagnostics.csv",diagnostics);save(out/"tail.csv",tails)
    print(f"Verified {sum(int(r['ops']) for r in trials):,} responses and {sum(int(r['publications']) for r in trials)} publications across 27 fresh runs.")
if __name__=="__main__":main()
