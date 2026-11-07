"""Summarize the completed run.py cohorts (Python standard library only)."""
import argparse
import csv
import json
import math
from pathlib import Path
from statistics import median


def trials(raw, stage, variants=('wal','dual')):
    result = []
    for run in json.loads((raw / stage / "runs.json").read_text()):
        assert run["status"] == "passed" and not run["state"]["OOMKilled"]
        _, variant, *fields, verified = run["summary"].split(",")
        n, ops, cap, elapsed, init, pin, size, sync, cp, logical, disk, rss0, hwm0, rss1, hwm1, reopen, recovery, count = map(int, fields)
        lines = (raw / stage / (run["label"] + ".log")).read_text().splitlines()
        requests = [line.split(",") for line in lines if line.startswith("SCALED_REQUEST,")]
        assert [int(q[3]) for q in requests] == list(range(ops))
        lat = sorted(int(q[4])/1e6 for q in requests)
        switches = [int(q[6])/1e6 for q in requests if int(q[6])]
        assert verified == "true" and cp == len(switches) == 3 and sync == ops + (3 if variant=='dirty' else 2)*cp
        result.append(dict(rows=n, variant=variant, rep=run["rep"], ops_s=ops*1e9/elapsed,
            p99_ms=lat[math.ceil(.99*ops)-1], max_ms=max(lat), checkpoint_ms=median(switches),
            syncs=sync, written_MiB=size/2**20, rss_end_MiB=rss1/2**20, hwm_MiB=hwm1/2**20,
            reopen_ms=reopen/1e6, logical_MiB=logical/2**20, allocated_MiB=disk/2**20,
            pin_ms=pin/1e6, verified_rows=count))
    assert sorted((r['rep'],r['variant']) for r in result)==sorted((rep,v) for rep in range(1,4) for v in variants)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("raw",type=Path)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    args.out.mkdir(parents=True,exist_ok=True)
    rows=trials(args.raw,"100k-final")+trials(args.raw,"1m-final")
    with (args.out/"trials.csv").open("w",newline="",encoding="utf8") as f:
        w=csv.DictWriter(f,rows[0].keys(),lineterminator="\n");w.writeheader();w.writerows(rows)
    groups=[[r for r in rows if r["rows"]==n and r["variant"]==v] for n in (100000,1000000) for v in ("wal","dual")]
    metrics=[("ops_s","Updates / second"),("p99_ms","Update p99 (ms)"),("max_ms","Slowest update (ms)"),("written_MiB","Application writes (MiB)"),("rss_end_MiB","End RSS (MiB)"),("reopen_ms","Warm reopen (ms)")]
    svg=['<svg xmlns="http://www.w3.org/2000/svg" width="920" height="540" viewBox="0 0 920 540" role="img"><title>Checkpoint-inclusive single-writer benchmark</title><rect width="920" height="540" fill="white"/><g font-family="sans-serif" fill="#17263a">']
    def label(x,y,s,size):
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(20,32,"Dual records and region reuse vs. current WAL",24)
    label(20,56,"2 CPUs / 4 GiB. Median of three runs, except slowest update (worst of three). Independent panel scales.",14)
    for i,(key,title) in enumerate(metrics):
        x=18+(i%3)*304;y=89+(i//3)*220;label(x,y,title,17)
        vals=[(max if key=="max_ms" else median)(r[key] for r in g) for g in groups]
        for j,(v,name) in enumerate(zip(vals,("100k WAL","100k dual","1M WAL","1M dual"))):
            yy=y+32+j*39;label(x,yy,name,12)
            width=v/max(vals)*135
            color=("#396494","#ae5429")[j%2]
            svg.append(f'<rect x="{x+68}" y="{yy-13}" width="{width:.3f}" height="15" fill="{color}"/>')
            label(x+73+width,yy,f"{v:,.2f}",12)
    svg.append('</g></svg>')
    (args.out/"comparison.svg").write_text('\n'.join(svg)+'\n',encoding="utf8")
    for g in groups:
        print(g[0]["rows"],g[0]["variant"],{k:round((max if k=="max_ms" else median)(r[k] for r in g),3) for k in g[0] if k not in ("rows","variant","rep")})


if __name__=="__main__":
    main()
