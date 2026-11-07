"""Plot released-interface results from the matched four-client product benchmark."""
import argparse
import csv
import gzip
import json
import math
from pathlib import Path
from statistics import median
from report_products import chart

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("raw",type=Path)
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    env=json.loads((args.raw/"environment.json").read_text())
    assert (env["rows"],env["ops_per_case"],env["reps"])==(100000,1024,3)
    digest={}
    for run in env["runs"]:
        verify=run["verification"][-1]
        assert run["status"]=="passed" and verify["stage"]=="reopen"
        assert verify["rows"]==100000 and verify["expected_sha256"]==verify["sha256"]
        digest.setdefault(run["rep"],set()).add(verify["sha256"])
    assert len(digest)==3 and all(len(v)==1 for v in digest.values())
    data=json.loads((args.raw/"results.json").read_text())
    names={"catdb-embedded":"catdb C ABI","sqlite":"SQLite","catdb-service":"catdb service","postgres":"PostgreSQL","mysql":"MySQL","oracle":"Oracle Free"}
    assert sorted((r["engine"],r["rep"]) for r in data)==sorted((k,i) for k in names for i in (1,2,3))
    for r in data:
        assert (r["clients"],r["workload"],r["completed"],len(r["errors"]))==(4,"update",1024,0)
        with gzip.open(args.raw/r["latency_file"],"rt") as f:lat=sorted(json.load(f))
        assert len(lat)==1024 and abs(lat[math.ceil(len(lat)*.99)-1]/1e6-r["p99_ms"])<1e-6
    rows=[]
    for k,name in names.items():
        runs=[r for r in data if r["engine"]==k]
        rows.append(dict(engine=name,ops_s=median(r["ops_s"] for r in runs),p99_ms=median(r["p99_ms"] for r in runs)))
    args.out.mkdir(parents=True,exist_ok=True)
    with (args.out/"products.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)
    chart(args.out/"embedded.svg","Embedded updates through Python drivers",rows[:2])
    chart(args.out/"service.svg","Service updates through Python drivers",rows[2:])
if __name__=="__main__":
    main()
