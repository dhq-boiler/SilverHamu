"""Join the original quiet subset by successful request ID to that run's kernel fsync."""
import csv
import gzip
import os
from pathlib import Path
import sys

BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"journal-identity"))
from identity import identity

def export(raw,prior,out):
    summaries=[]
    for mode in ("standard","fifo","aged"):
        with gzip.open(prior/f"3-{mode}.csv.gz","rt") as f:
            quiet={int(r["request"]):int(r["sync_ns"]) for r in csv.DictReader(f) if int(r["prior_B"])==int(r["during_B"])==0}
        _,calls=identity(raw/f"3-{mode}-trace",("bio",))
        wal={c["request"]:c for c in calls if c["phase"]=="wal"}
        assert len(wal)==16384 and quiet.keys()<=wal.keys()
        rows=[]
        for key,t in sorted(quiet.items()):
            c=wal[key]
            assert 0<=c["journal_ns"]<=c["sync_ns"]<=t+4000
            rows.append(dict(request=key,app_ns=t,syscall_ns=c["sync_ns"],journal_ns=c["journal_ns"],
                             during_journal_commit_ns=c["during_commit_ns"],classification=c["status"]))
        with gzip.open(out/f"original-{mode}-quiet.csv.gz","wt",newline="") as f:
            w=csv.DictWriter(f,list(rows[0]));w.writeheader();w.writerows(rows)
        sums={k.replace("_ns","_ms"):sum(r[k] for r in rows)/len(rows)/1e6 for k in ("app_ns","syscall_ns","journal_ns","during_journal_commit_ns")}
        summaries.append(dict(mode=mode,quiet=len(rows),**sums))
    with (out/"original.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,list(summaries[0]));w.writeheader();w.writerows(summaries)
    return summaries

if __name__=="__main__":
    print(export(*map(Path,sys.argv[1:])))
