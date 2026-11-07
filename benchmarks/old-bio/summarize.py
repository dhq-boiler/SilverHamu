"""Summarize application outcomes separately from kernel capture eligibility."""
import csv
import gzip
import json
from pathlib import Path
import re
from statistics import mean
import sys
from analyze_bio import records
from report_independent import analyze

def summarize(root,out):
    rows=[]
    for phase in ("pilot","lean","full"):
        for folder in sorted((root/phase).glob("*-aged-*") if phase!="full" else (root/phase).glob("*-trace")):
            rep,mode,kind=folder.name.split("-")
            log=(folder/"app.log").read_text()
            ops,cap=(16384,256) if phase=="full" else (512,32)
            stats=analyze(log,mode,int(rep),ops,cap)[0]
            row=dict(phase=phase,rep=rep,mode=mode,kind=kind,
                     ops=ops,publications=(ops-1)//cap,updates_s=stats["updates_s"],p99_ms=stats["p99_ms"],max_ms=stats["max_ms"])
            if kind=="trace":
                m=json.loads((folder/"meta.json").read_text())
                for probe in ("bio","completed"):
                    entry,= [s for s in m["probe_profile"] if s.strip().startswith(probe+"_")]
                    row[probe+"_miss"]=int(entry.split()[-1])
                row["ring_loss"]=sum(int(n) for v in m["stats"].values() for n in re.findall(r"^(?:overrun|commit overrun|dropped events):\s*(\d+)",v,re.M))
                row["released"]=m["cleaned"]
                prefix="" if (folder/"pairing.json").exists() else "upper-"
                if (folder/(prefix+"pairing.json")).exists():
                    row.update(json.loads((folder/(prefix+"pairing.json")).read_text()))
                    with gzip.open(folder/(prefix+"calls.csv.gz"),"rt") as f:
                        calls=list(csv.DictReader(f))
                    quiet=[r for r in calls if int(r["prior_n"])==int(r["during_n"])==0]
                    for name,part in (("quiet",quiet),("pending",[r for r in quiet if int(r["old_pending_n"])]),("clear",[r for r in quiet if not int(r["old_pending_n"])])):
                        row[name+"_ms"]=mean(int(r["sync_ns"]) for r in part)/1e6 if part else ""
                    row["all_old"]=sum(int(r["old_pending_n"])>0 for r in calls)
                    row["quiet_old_sync_s"]=sum(int(r["sync_ns"]) for r in quiet if int(r["old_pending_n"]))/1e9
                    row["quiet_sync_s"]=sum(int(r["sync_ns"]) for r in quiet)/1e9
                    if "old_overlap_ns" in calls[0]:
                        row["quiet_old_overlap_s"]=sum(int(r["old_overlap_ns"]) for r in quiet)/1e9
            rows.append(row)
    fields=list(dict.fromkeys(k for r in rows for k in r))
    with (out/"summary.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,fields);w.writeheader();w.writerows(rows)
    return rows
if __name__=="__main__":
    summarize(Path(sys.argv[1]),Path(sys.argv[2]))
