"""Scheduled and continuous writes using the same checkpoint workload and observer."""
from collections import defaultdict
import argparse
import csv
import gzip
import json
import math
import os
from pathlib import Path
import re
import sys

BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"ext4-checkpoint-bytes"))
from run import one,call
sys.path.insert(0,str(BENCH/"scaled-gate"))
from report_independent import records,overlap,save
from report_equal import handoffs

CASES={"single10":(1,10,1024),"four20":(4,20,1024),"continuous":(4,0,16384)}
MODES=("standard","fifo","aged")

def percentile(v):
    return sorted(v)[math.ceil(.99*len(v))-1]/1e6

def responses(attempts,origin,rate,ops):
    groups=defaultdict(list)
    for a in attempts:groups[a[0]].append(a)
    assert sorted(groups)==list(range(ops))
    rows=[]
    for i,tries in sorted(groups.items()):
        sent,end=tries[0][1],tries[-1][9]
        assert all(a[1]==sent and a[1]<=a[2]<=a[3]<=a[4]<=a[5]<=a[9] for a in tries)
        assert all(not a[-1] for a in tries[:-1]) and tries[-1][-1]
        assert all(a[9]<=b[2] for a,b in zip(tries,tries[1:]))
        due=origin+i*10**9//rate if rate else sent
        assert due<=sent<=end
        rows.append((i,due,sent,end,len(tries)-1,tries[-1][-1]))
    seq=sorted(r[-1] for r in rows)
    assert seq==list(range(seq[0],seq[0]+ops))
    return rows

def accounting(log,ops,lanes,allow_idle=False):
    h=handoffs(log,ops,256,lanes,allow_idle=allow_idle)
    a=records(log,"ATTEMPT");holds=records(log,"HOLD")
    lo=min(r[1] for r in a);hi=max(r[9] for r in a)
    wal=sum(overlap(r[6],r[7],lo,hi) for r in a if r[-1])/1e9
    writers=sum(overlap(r[2],r[3],lo,hi) for r in holds if r[0]!=4)/1e9
    cp=sum(overlap(r[2],r[3],lo,hi) for r in holds if r[0]==4)/1e9
    gap=h["active_envelope_ms"]/1000
    remaining=(hi-lo)/1e9-writers-cp-gap
    assert remaining>=-1e-7 and writers>=wal
    return dict(response_s=(hi-lo)/1e9,wal_s=wal,handoff_s=gap,checkpoint_hold_s=cp,
                other_writer_s=writers-wal,remaining_s=remaining,drain_s=h["observable_drained_ms"]/1000,
                contended=h["contended"],handoff_p99_us=h["p99_envelope_us"])

def analyze(log,case,mode,rep,out):
    lanes,rate,ops=CASES[case]
    assert records(log,"INDEPENDENT_OK")==[(ops,)]
    assert [r[0] for r in records(log,"PUBLICATION")]==list(range(1,ops//256))
    anchors=records(log,"LOAD_SCHEDULE");assert len(anchors)==1
    origin,l,r=anchors[0];assert (l,r)==(lanes,rate)
    rows=responses(records(log,"ATTEMPT"),origin,rate,ops)
    with gzip.open(out/f"{case}-{mode}-{rep}-requests.csv.gz","wt",newline="") as f:
        w=csv.writer(f,lineterminator="\n");w.writerow("id due_ns sent_ns end_ns retries version".split());w.writerows(rows)
    acct=accounting(log,ops,lanes,allow_idle=bool(rate))
    service=[end-sent for _,due,sent,end,*_ in rows]
    scheduled=[end-due for _,due,sent,end,*_ in rows]
    lag=[sent-due for _,due,sent,end,*_ in rows]
    return dict(case=case,mode=mode,rep=rep,ops=ops,publications=ops//256-1,
                updates_s=ops/acct["response_s"],drained_rate=ops/acct["drain_s"],
                scheduled_p99_ms=percentile(scheduled),sent_p99_ms=percentile(service),
                max_scheduled_ms=max(scheduled)/1e6,max_sent_ms=max(service)/1e6,
                send_lag_p99_ms=percentile(lag),retries=sum(r[-2] for r in rows),**acct)

def main():
    p=argparse.ArgumentParser();p.add_argument("raw",type=Path);p.add_argument("out",type=Path);args=p.parse_args()
    root=args.raw.resolve();out=args.out.resolve();root.mkdir(parents=True,exist_ok=False);out.mkdir(parents=True,exist_ok=False)
    image="catdb-aged-load";(out/"images.json").write_text(json.dumps({n:call("docker","image","inspect","--format","{{.Id}}",n) for n in (image,"catdb-fifo-products")},indent=2))
    all_rows=[];quality=[];excluded=[]
    for rep in range(1,4):
        for j in range(3):
            case=list(CASES)[(j+rep-1)%3]
            lanes,rate,ops=CASES[case]
            dest=root/case;dest.mkdir(exist_ok=True)
            for k in range(3):
                mode=MODES[(k+list(CASES).index(case)+rep-1)%3]
                folder=dest/f"{rep}-{mode}-trace"
                for attempt in range(1,4):
                    one(dest,rep,mode,True,False,image=image,range_writeback=True,writers=lanes,rate=rate)
                    meta=json.loads((folder/"meta.json").read_text())
                    lost=sum(int(v) for st in meta["stats"].values() for v in re.findall(r"^(?:overrun|commit overrun|dropped events):\s*(\d+)",st,re.M))
                    miss=sum(int(x.split()[-1]) for x in meta["probe_profile"])
                    assert meta["cleaned"]
                    log=(folder/"app.log").read_text()
                    if not (lost or miss):break
                    bad=dest/f"{rep}-{mode}-excluded-{attempt}"
                    assert dest.resolve() in folder.resolve().parents and dest.resolve() in bad.resolve().parents and not bad.exists()
                    folder.rename(bad)
                    excluded.append(dict(case=case,rep=rep,mode=mode,attempt=attempt,ring_loss=lost,probe_misses=miss))
                    save(out/"excluded.csv",excluded)
                else:raise RuntimeError("repeated trace loss")
                row=analyze(log,case,mode,rep,out)
                # Export only the workload markers needed for independent reconstruction.
                with gzip.open(out/f"{case}-{mode}-{rep}-markers.txt.gz","wt") as f:
                    for line in log.splitlines():
                        if line.startswith(("ATTEMPT,","HOLD,","PUBLICATION,","LOAD_SCHEDULE,","INDEPENDENT_OK,")):f.write(line+"\n")
                all_rows.append(row);quality.append(dict(case=case,rep=rep,mode=mode,kernel=meta["kernel"],device=meta["device"],ring_loss=lost,probe_misses=miss))
                save(out/"trials.csv",all_rows);save(out/"quality.csv",quality)
                print("MEASURED",case,rep,mode,round(row["updates_s"],2),round(row["scheduled_p99_ms"],2),flush=True)
if __name__=="__main__":main()
