"""Plot the four-client update case. Native-test samples have their own panel."""
import argparse
import csv
import gzip
import json
import math
from pathlib import Path
from statistics import median

def chart(path,title,groups):
    height=112+len(groups)*39
    svg=[f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 900 {height}" role="img"><title>{title}</title><rect width="900" height="{height}" fill="white"/><g font-family="sans-serif" fill="#17283b">']
    def label(x,y,s,size):svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(12,27,title,21)
    label(12,50,"100k rows, 256 B, four clients, 1,024 durable updates; median of three trials.",14)
    for i,(key,text,color) in enumerate([("ops_s","Updates / second","#346a91"),("p99_ms","p99 (ms)","#286f59")]):
        x=200+345*i;label(x,78,text,16)
        peak=max(g[key] for g in groups)
        for j,g in enumerate(groups):
            y=111+39*j
            if i==0:label(12,y,g["engine"],14)
            w=g[key]*225/peak
            svg.append(f'<rect x="{x}" y="{y-12}" width="{w:.2f}" height="14" fill="{color}"/>')
            label(x+w+6,y,f"{g[key]:,.2f}",13)
    path.write_text("\n".join(svg+["</g></svg>"])+"\n",encoding="utf8")

def main():
    p=argparse.ArgumentParser()
    for arg in ("products","native","out"):p.add_argument(arg,type=Path)
    a=p.parse_args()
    rows=json.loads((a.products/"results.json").read_text())
    env=json.loads((a.products/"environment.json").read_text())
    assert env["rows"]==100000 and env["ops_per_case"]==1024 and env["reps"]==3
    checks={}
    for run in env["runs"]:
        assert run["status"]=="passed"
        v=run["verification"][-1]
        assert v["stage"]=="reopen" and v["rows"]==100000 and v["sha256"]==v["expected_sha256"]
        checks.setdefault(run["rep"],set()).add(v["sha256"])
    assert len(checks)==3 and all(len(v)==1 for v in checks.values())
    for r in rows:
        assert (r["workload"],r["clients"],r["completed"],len(r["errors"]))==("update",4,1024,0)
        with gzip.open(a.products/r["latency_file"],"rt") as f:samples=sorted(json.load(f))
        assert len(samples)==1024 and abs(samples[math.ceil(len(samples)*.99)-1]/1e6-r["p99_ms"])<1e-6
    native=json.loads((a.native/"results.json").read_text())
    assert all(r["rows_verified"]==100000 for r in native)
    assert all(r["expected_sha256"] in checks[r["rep"]] for r in native)
    rows+=native
    engines={"combine":"catdb prototype / before","logreuse":"catdb prototype / after","catdb-embedded":"catdb embedded (C ABI)","sqlite":"SQLite","catdb-service":"catdb service","postgres":"PostgreSQL","mysql":"MySQL","oracle":"Oracle Free"}
    assert sorted((r["engine"],r["rep"]) for r in rows)==sorted((e,i) for e in engines for i in (1,2,3))
    summary={}
    for e,name in engines.items():
        runs=[r for r in rows if r["engine"]==e]
        summary[e]=dict(engine=name,ops_s=median(r["ops_s"] for r in runs),p99_ms=median(r["p99_ms"] for r in runs))
    a.out.mkdir(exist_ok=True,parents=True)
    with (a.out/"product-summary.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,["engine","ops_s","p99_ms"],lineterminator="\n");w.writeheader();w.writerows(summary.values())
    for name,title,order in [("embedded","Embedded updates (Rust prototype; Python for C ABI and SQLite)",["combine","logreuse","catdb-embedded","sqlite"]),("service","Service update comparison (Python clients)",["catdb-service","postgres","mysql","oracle"]),("native","Experimental catdb storage paths (native Rust)",["combine","logreuse"])]:
        chart(a.out/(name+".svg"),title,[summary[e] for e in order])
if __name__=="__main__":main()
