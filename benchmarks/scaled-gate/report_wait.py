"""Reproduce scheduling-counter attribution and the matched FIFO comparison."""
import argparse
import csv
import json
import math
from pathlib import Path
from statistics import median
from collections import defaultdict

def parse(raw,run):
    mode=run["mode"]
    assert run["status"]=="passed" and not run["state"]["OOMKilled"]
    _,backend,rows,ops,elapsed,rss,verified=run["summary"].split(",")
    assert verified=="verified" and backend==("wal" if mode=="wal" else "dirty")
    lane_bounds={}
    requests=[]
    checkpoints=[]
    spans=defaultdict(list)
    for line in (raw/(run["label"]+".log")).read_text().splitlines():
        fields=line.split(",")
        if fields[0]=="WAIT_LANE":
            lane,cpu,ready=map(int,fields[1:]);assert lane not in lane_bounds;lane_bounds[lane]=(cpu,ready)
        elif fields[0]=="SCALED_REQUEST":
            requests.append(tuple(map(int,fields[2:])))
        elif fields[0]=="FOUR_CHECKPOINT":
            checkpoints.append(tuple(map(int,fields[2:])))
        elif fields[0]=="WAIT_SPAN":
            spans[int(fields[1])].append((fields[2],*map(int,fields[3:])))
    assert sorted(x[0] for x in requests)==list(range(int(ops)))
    assert max(x[2] for x in requests)-min(x[2] for x in requests)+1==len(set(x[2] for x in requests))==int(ops)
    assert len(checkpoints)==3 and all(b>a for a,b in checkpoints)
    assert all(x[1]==x[0]%4 for x in requests)
    def overlap(a,b):
        return sum(max(0,min(b,y)-max(a,x)) for x,y in checkpoints)
    slow=max(requests,key=lambda x:x[4]-x[3])
    ident,lane,_,a,b,retries=slow
    latency=sorted((r[4]-r[3])/1e6 for r in requests)
    result=dict(run=run["label"],mode=mode,tx_s=int(ops)*1e9/int(elapsed),p99_ms=latency[math.ceil(.99*len(latency))-1],
                max_ms=latency[-1],slow_id=ident,slow_overlaps_ms=overlap(a,b)/1e6,
                others_during_slowest=sum(r[1]!=lane and a<r[4]<b for r in requests),
                conflicts=sum(r[5] for r in requests),rss_MiB=int(rss)/2**20)
    detail=[]
    if mode=="light":
        assert sorted(lane_bounds)==[0,1,2,3]
        result["runnable_upper_ms"]=lane_bounds[lane][1]/1e6
    if mode in ("observed","light"):
        assert len(spans)==int(ops)
        for idx,_,_,start,end,retry in requests:
            trace=spans[idx]
            assert [s[0] for s in trace]==["prepare","admission","writer","commit"]*(retry+1)
            assert start<=trace[0][1] and trace[-1][2]<=end
            assert all(x[2]==y[1] for x,y in zip(trace,trace[1:]))
            assert all(y>=x and cpu>=0 and ready>=0 for _,x,y,cpu,ready in trace)
        def add(stage,x,y,cpu=None,ready=None):
            wall=y-x
            cp=overlap(x,y)
            outside=wall-cp
            # An interval straddling a checkpoint cannot be split precisely using two counter samples.
            # Keep signed imbalance visible instead of silently treating counter skew as blocked I/O.
            base=dict(run=run["label"],id=ident,stage=stage,start_ns=x,end_ns=y,wall_ms=wall/1e6,
                      checkpoint_ms=cp/1e6,outside_ms=outside/1e6)
            if cpu is not None and mode=="observed":
                base.update(cpu_ms=cpu/1e6,runnable_ms=ready/1e6,residual_ms=(wall-cpu-ready)/1e6,
                            runnable_outside_low_ms=max(0,ready-cp)/1e6,
                            runnable_outside_high_ms=min(outside,ready)/1e6)
            detail.append(base)
        trace=spans[ident]
        add("marker edge",a,trace[0][1])
        for s in trace:add(*s)
        add("marker edge",trace[-1][2],b)
        assert abs(sum(x["outside_ms"] for x in detail)+result["slow_overlaps_ms"]-result["max_ms"])<1e-5
    else:
        assert not spans
    return result,detail

def write_csv(path,rows):
    with path.open("w",newline="") as f:
        w=csv.DictWriter(f,dict.fromkeys(k for r in rows for k in r),lineterminator="\n")
        w.writeheader()
        w.writerows(rows)

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    args.out.mkdir(parents=True,exist_ok=True)
    for stage in ("pilot","diagnostic","light","1m"):
        raw=args.raw/stage
        rows=[];details=[]
        for run in json.loads((raw/"runs.json").read_text()):
            row,spans=parse(raw,run)
            rows.append(row);details.extend(spans)
        if stage=="1m":
            assert sorted((r["mode"],r["run"].split("-")[0]) for r in rows)==sorted((m,str(i)) for m in ("wal","dirty","fifo") for i in (1,2,3))
        elif stage=="light":assert [r["mode"] for r in rows]==["dirty","light","light","dirty"]
        else:assert [r["mode"] for r in rows]==["dirty","observed","observed","dirty"]
        write_csv(args.out/(stage+".csv"),rows)
        if details:write_csv(args.out/(stage+"-slow-spans.csv"),details)
    labels=("WAL","Delta: mutex","Delta: FIFO")
    metrics=(("max_ms","Worst request (ms)"),("p99_ms","Request p99 (ms)"),("tx_s","Transactions per second"))
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 970 245" role="img"><title>Four writers and checkpoint stalls</title><rect width="970" height="245" fill="white"/><g font-family="sans-serif" fill="#18293d">']
    def label(x,y,s,size):svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(16,32,"Waiting for a publication turn under four concurrent writers",23)
    label(16,58,"1M rows, 2 CPUs, 4 GiB. Worst across three trials. Throughput and p99 are medians of trial statistics.",14)
    for i,(k,title) in enumerate(metrics):
        x=16+i*321;label(x,94,title,17)
        vals=[(max if k=="max_ms" else median)(r[k] for r in rows if r["mode"]==m) for m in ("wal","dirty","fifo")]
        for j,(name,v,color) in enumerate(zip(labels,vals,("#396494","#a54c31","#28795d"))):
            y=130+j*38;label(x,y,name,12);width=145*v/max(vals)
            svg.append(f'<rect x="{x+82}" y="{y-12}" width="{width:.2f}" height="13" fill="{color}"/>')
            label(x+88+width,y,f"{v:,.2f}",12)
    (args.out/"comparison.svg").write_text("\n".join(svg+["</g></svg>"])+"\n")
if __name__=="__main__":
    main()
