"""Join tagged fsyncs to unique journal commit epochs on the workload device."""
import bisect
import gzip
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

import os
BENCH = Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"wal-sync-waits"))
from partition import LINE, analyze, normalize, records, syscall_segments

def classify(markers, facts, waits, journal, begin, end):
    starts = markers.get("jbd2_start_commit", [])
    ends = markers.get("jbd2_end_commit", [])
    if len(starts) != 1 or len(ends) != 1 or starts[0] > ends[0]:
        return None, "unresolved"
    completed = ends[0]
    first = min((t for t,k,_ in facts if k != "ext4"), default=end)
    if completed < first:
        return completed, "already_ended"
    if any(t <= s <= completed <= e for t,_ in waits for s,e in journal):
        return completed, "blocked_at_end"
    if begin <= completed <= end:
        return completed, "observed_end"
    return completed, "unresolved"

def identity(folder):
    meta = json.loads((folder / "meta.json").read_text())
    tag = meta["tag"]
    assert len(meta["probe_profile"]) == 2
    assert all(int(line.split()[-1]) == 0 for line in meta["probe_profile"])
    assert {line.split()[0] for line in meta["probe_profile"]} == {"target_"+tag, "waiting_"+tag}
    events = normalize(folder, tag, 5)
    calls = syscall_segments(events)
    app = (folder / "app.log").read_text()
    anchors = records(app, "OS_CLOCK")
    assert len(anchors) == 2 and all(hi-lo == w < 2000 for w,lo,hi in anchors)
    low, high = max(x[1] for x in anchors), min(x[2] for x in anchors)
    assert low <= high + 1000
    origin = (low + high) // 2
    commits = defaultdict(lambda: defaultdict(list))
    extra = defaultdict(list)
    with gzip.open(folder / "trace.gz", "rt") as stream:
        for line in stream:
            match = LINE.match(line)
            if not match:
                continue
            comm, _, _, sec, frac, body = match.groups()
            t = int(sec)*10**9 + int(frac.ljust(9,"0"))
            if body.startswith(("jbd2_start_commit:", "jbd2_end_commit:")):
                dev, tx = re.search(r"dev ([\d,]+) transaction (\d+)",body).groups()
                assert dev == meta["device"]
                commits[dev, int(tx)][body.split(":")[0]].append(t)
            elif body.startswith(("ext4_sync_file_enter:", "target_"+tag+":", "waiting_"+tag+":")):
                comm = comm.strip()
                assert comm[:-1] == "cw"+tag
                field = body.split(":")[0].split("_")[0]
                value = re.search(r"dev ([\d,]+)", body)[1] if field=="ext4" else int(body.split("tx=")[1])
                extra[int(comm[-1])].append((t,field,value))
    attempts = defaultdict(list)
    for a in records(app, "ATTEMPT"):
        if a[-1]:
            attempts[a[0]%4].append(a)
    cps = records(app, "PUBLICATION")
    phases = [(origin+c[i], origin+c[j], label) for c in cps for i,j,label in
              ((4,5,"snapshot"), (7,8,"tail"), (8,9,"replace"))]
    data = []
    for lane, batch in calls.items():
        delta = extra[lane]
        times = [row[0] for row in delta]
        if lane < 4:
            assert len(batch) == len(attempts[lane])
        for i, pieces in enumerate(batch):
            begin, end = pieces[0][0], pieces[-1][1]
            if lane < 4:
                a = attempts[lane][i]
                assert begin >= origin+a[6]-2000 and end <= origin+a[7]+2000
                phase = "wal"
            else:
                match = [label for lo,hi,label in phases if lo-2000<=begin and end<=hi+2000]
                assert len(match)<=1
                phase = match[0] if match else "other_cp"
            facts = delta[bisect.bisect_left(times,begin):bisect.bisect_right(times,end)]
            devices = [v for _,k,v in facts if k=="ext4"]
            assert devices == [meta["device"]]
            targets = {v for _,k,v in facts if k=="target"}
            waits = [(t,v) for t,k,v in facts if k=="waiting"]
            ids = targets.union(v for _,v in waits)
            journal = [(s,e) for s,e,k in pieces if k=="journal"]
            tx = next(iter(ids)) if len(ids)==1 else None
            completed, status = classify(commits.get((devices[0], tx), {}), facts, waits, journal, begin, end)
            commit_start=commits[devices[0],tx]["jbd2_start_commit"][0] if status=="blocked_at_end" else None
            before=during=after=0
            if commit_start is not None:
                before=sum(max(0,min(e,commit_start)-s) for s,e in journal)
                during=sum(max(0,min(e,completed)-max(s,commit_start)) for s,e in journal)
                after=sum(max(0,e-max(s,completed)) for s,e in journal)
                assert before+during+after==sum(e-s for s,e in journal)
            data.append(dict(lane=lane,index=i,phase=phase,tx=tx,completed=completed,status=status,
                             journal_ns=sum(e-s for s,e in journal),sync_ns=end-begin,
                             wait_start=next((s for s,e in journal if completed is not None and s<=completed<=e),None),
                             commit_start=commit_start,before_start_ns=before,during_commit_ns=during,after_end_ns=after,
                             begin=begin,end=end, shared_phase="",overlap_ns=0,request=a[0] if lane<4 else None))
    assert sum(d["phase"]=="snapshot" for d in data)==len(cps)
    peers = {(d["tx"],d["completed"]):d for d in data if d["lane"]==4 and d["status"]=="blocked_at_end"}
    overlap=shared=calls_shared=0
    for d in data:
        if d["phase"]!="wal" or d["status"]!="blocked_at_end":
            continue
        peer=peers.get((d["tx"],d["completed"]))
        if peer:
            d["shared_phase"]=peer["phase"]
            d["overlap_ns"]=d["completed"]-max(d["wait_start"],peer["wait_start"])
            shared+=d["journal_ns"]
            calls_shared+=1
            overlap+=d["completed"]-max(d["wait_start"],peer["wait_start"])
    wal=[d for d in data if d["phase"]=="wal"]
    cp_targets = {(d["tx"],d["completed"]) for d in data if d["lane"]==4 and d["completed"] is not None}
    wal_with_target = [d for d in wal if (d["tx"],d["completed"]) in cp_targets]
    return dict(before_start_s=sum(d["before_start_ns"] for d in wal)/1e9,
                during_commit_s=sum(d["during_commit_ns"] for d in wal)/1e9,
                after_end_s=sum(d["after_end_ns"] for d in wal)/1e9,
                same_epoch=len(wal_with_target), shared_snapshot=sum(d["shared_phase"]=="snapshot" for d in wal),
                shared_tail=sum(d["shared_phase"]=="tail" for d in wal),
                shared_replace=sum(d["shared_phase"]=="replace" for d in wal),
                shared_other=sum(d["shared_phase"]=="other_cp" for d in wal), wal=len(wal),snapshot=len(cps),shared=calls_shared,shared_call_sleep_s=shared/1e9,
                simultaneous_s=overlap/1e9,journal_s=sum(d["journal_ns"] for d in wal)/1e9,
                sync_s=sum(d["sync_ns"] for d in wal)/1e9,
                wal_resolved=sum(d["status"]=="blocked_at_end" for d in wal),
                cp_blocked_epochs=len(peers), shared_epochs=len({(d["tx"],d["completed"]) for d in wal if d["status"]=="blocked_at_end" and (d["tx"],d["completed"]) in peers}), already=sum(d["status"]=="already_ended" for d in wal), unresolved=sum(d["status"]=="unresolved" for d in wal)), data

if __name__=="__main__":
    for arg in sys.argv[1:]:
        print(arg,identity(Path(arg))[0],flush=True)
