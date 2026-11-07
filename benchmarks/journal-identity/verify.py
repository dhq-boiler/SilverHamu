"""Verify the public evidence with a separate aggregation of per-call records."""
import csv
import gzip
import math
import sys
from collections import Counter
from pathlib import Path
from xml.etree.ElementTree import parse

def verify(root):
    def load(name):
        with (root/name).open(newline="") as stream:
            return list(csv.DictReader(stream))
    for name in ("performance.svg","shared.svg"):
        assert parse(root/name).getroot().tag.endswith("svg")
    assert len(load("pilot.csv"))==8
    for suite,expected in [("quality.csv",6),("pilot-quality.csv",4)]:
        quality=load(suite)
        assert len(quality)==expected and all(r["cleaned"]=="True" and int(r["cpu_buffers"])>0 for r in quality)
    trials=load("trials.csv")
    assert len(trials)==6
    for r in trials:
        mode,rep=r["mode"],r["rep"]
        with gzip.open(root/f"{rep}-{mode}-trace.csv.gz","rt") as f:
            rows=list(csv.DictReader(f))
        wal=[x for x in rows if x["phase"]=="wal"]
        assert sorted(int(x["request"]) for x in wal)==list(range(16384))
        assert sum(x["phase"]=="snapshot" for x in rows)==63
        ready={(x["tx"],x["completed"]) for x in rows if x["lane"]=="4" and x["status"]=="blocked_at_end"}
        assert all(sum(int(x[k]) for k in ("before_start_ns","during_commit_ns","after_end_ns"))==int(x["journal_ns"]) for x in wal if x["status"]=="blocked_at_end")
        shared=[x for x in wal if x["status"]=="blocked_at_end" and (x["tx"],x["completed"]) in ready]
        assert len(shared)==int(r["shared"])
        for key,source,column in [("shared_call_sleep_s",shared,"journal_ns"),("journal_s",wal,"journal_ns"),
                                   ("simultaneous_s",shared,"overlap_ns")]:
            assert math.isclose(sum(int(x[column]) for x in source)/1e9,float(r[key]),abs_tol=1e-9)
    print("Verified every request, publication count and shared-wait total in six trials.")

if __name__=="__main__":
    verify(Path(sys.argv[1]))
