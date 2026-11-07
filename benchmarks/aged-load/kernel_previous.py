"""Attribute the original WAL intervals to the guest journal wait when evidence matches."""
import argparse
import csv
import gzip
from pathlib import Path
import os
import sys

BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"journal-identity"))
from identity import identity
from report_independent import save

if __name__=="__main__":
    p=argparse.ArgumentParser();p.add_argument("raw",type=Path);p.add_argument("out",type=Path);args=p.parse_args()
    out=[]
    for folder in sorted(args.raw.glob("*-trace")):
        rep,mode,_=folder.name.split("-")
        stats,calls=identity(folder,("bio",))
        keys=("request","sync_ns","journal_ns","before_start_ns","during_commit_ns","after_end_ns","status")
        with gzip.open(args.out/f"previous-{mode}-{rep}-wal.csv.gz","wt",newline="") as f:
            writer=csv.DictWriter(f,keys,lineterminator="\n",extrasaction="ignore");writer.writeheader()
            writer.writerows(c for c in calls if c["phase"]=="wal")
        out.append(dict(rep=rep,mode=mode,wal=stats["wal"],resolved=stats["wal_resolved"],syscall_s=stats["sync_s"],
                        journal_sleep_s=stats["journal_s"],before_start_s=stats["before_start_s"],
                        during_commit_s=stats["during_commit_s"],after_end_s=stats["after_end_s"]))
    assert len(out)==9
    save(args.out/"previous-kernel.csv",out)
