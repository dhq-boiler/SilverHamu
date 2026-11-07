"""Map fsyncs to unique writer identities, then partition guest time on one monotonic clock."""
import argparse,bisect,csv,gzip,json,os,re,sys
from collections import Counter,defaultdict
from pathlib import Path
from statistics import median
BENCH=Path(os.environ.get("CATDB_BENCH_ROOT",Path(__file__).resolve().parents[1]))
sys.path.insert(0,str(BENCH/"scaled-gate"))
from report_independent import analyze,records,save
sys.path.insert(0,str(BENCH/"sync-os"))
from analyze import LINE

KINDS=("writeback","journal","io_other","blocked_unknown","wake_wait","preempt_wait","scheduled","outside_syscall")
PHASES=("prefix","snapshot_write","snapshot_sync","finalize","outside")

def stack_kind(frames):
    if any(s.startswith("jbd2_log_wait_commit") for s in frames):return "journal"
    if any(s.startswith(("folio_wait_writeback","wait_on_page_writeback","__filemap_fdatawait_range")) for s in frames):return "writeback"
    if any(s.startswith("io_schedule") for s in frames):return "io_other"
    return "blocked_unknown"

def check_metadata(folder):
    meta=json.loads((folder/"meta.json").read_text())
    assert meta["cleaned"] and meta["stats"]
    for stats in meta["stats"].values():
        for field in ("overrun","commit overrun","dropped events"):
            assert int(re.search("^"+field+r":\s*(\d+)",stats,re.M)[1])==0,"lost trace events"

def normalize(folder,tag):
    check_metadata(folder)
    assert json.loads((folder/"meta.json").read_text())["tag"]==tag
    names={f"cw{tag}{i}":i for i in range(4)}
    out=[];latest={};frames=[];stack=None;ids={}
    def add(t,comm,pid,kind,cpu):
        lane=names[comm];assert ids.setdefault(lane,pid)==pid
        out.append([t,lane,pid,kind])
        latest[(cpu,pid)]=len(out)-1
    with gzip.open(folder/"trace.gz","rt") as stream:
        for line in stream:
            if line.startswith(" => "):
                if stack is not None:frames.append(line[4:].strip())
                continue
            if stack is not None:
                out[stack][3]=stack_kind(frames)
                stack=None;frames=[]
            m=LINE.match(line)
            if not m:
                assert not line.strip() or line.startswith("#"),"unexpected trace line"
                continue
            comm,pid,cpu,sec,frac,body=m.groups()
            t=int(sec)*10**9+int(frac.ljust(9,"0"));pid=int(pid);cpu=int(cpu);comm=comm.strip()
            if body.startswith("sys_fsync(fd:"):
                add(t,comm,pid,"enter",cpu)
            elif body.startswith("sys_fsync ->"):
                assert int(body.split("->")[1].strip(),0)==0
                add(t,comm,pid,"exit",cpu)
            elif body.startswith("sched_switch:"):
                x=re.search(r'prev_comm=(.*?) prev_pid=(\d+).*?prev_state=(\S+) ==> next_comm=(.*?) next_pid=(\d+)',body)
                assert x
                old,oldpid,state,new,newpid=x.groups()
                if old in names:add(t,old,int(oldpid),"preempt" if state.startswith("R") else "blocked_unknown",cpu)
                if new in names:add(t,new,int(newpid),"in",cpu)
            elif body.startswith("sched_wakeup:"):
                x=re.search(r'comm=(.*?) pid=(\d+)',body);add(t,x[1],int(x[2]),"wake",cpu)
            elif body=="<stack trace>":
                k=latest.get((cpu,pid))
                if k is not None and out[k][3]=="blocked_unknown":stack=k
    if stack is not None:out[stack][3]=stack_kind(frames)
    assert len(set(ids.values()))==4
    return out

def syscall_segments(events,diagnostics=None):
    calls=defaultdict(list);active={}
    for t,lane,pid,k in events:
        if k=="enter":
            assert lane not in active
            active[lane]=[t,"scheduled",[]]
            continue
        if lane not in active:
            assert k!="exit","fsync exit without entry"
            continue
        last,state,pieces=active[lane]
        assert t>=last
        if k=="exit":
            assert state=="scheduled"
            pieces.append((last,t,state));calls[lane].append(pieces);del active[lane];continue
        if k=="wake":
            if state=="scheduled":
                # A remote wake can race a task about to sleep; it has not switched out.
                if diagnostics is not None:diagnostics["wake_while_scheduled"]+=1
                continue
            assert state in ("writeback","journal","io_other","blocked_unknown"),(lane,state,k)
            nxt="wake_wait"
        elif k=="in":
            assert state in ("wake_wait","preempt_wait"),(lane,state,k)
            nxt="scheduled"
        else:
            assert state=="scheduled",(lane,state,k)
            nxt="preempt_wait" if k=="preempt" else k
        pieces.append((last,t,state))
        active[lane]=[t,nxt,pieces]
    assert not active
    return calls

def allocate(log,events):
    anchors=records(log,"OS_CLOCK");assert len(anchors)==2 and all(w==hi-lo and w<2000 for w,lo,hi in anchors)
    lo=max(x[1] for x in anchors);hi=min(x[2] for x in anchors)
    assert lo<=hi+1000,"clock offset disagreement"
    origin=(lo+hi)//2
    ids={int(l):(int(t),name) for l,t,name in re.findall(r"OS_LANE,(\d+),(\d+),(cw[0-9a-f]+)",log)}
    assert sorted(ids)==list(range(4)) and len({v[0] for v in ids.values()})==4
    assert len(re.findall(r"OS_LANE,",log))==4
    assert len({v[1][:-1] for v in ids.values()})==1 and all(v[1].endswith(str(l)) for l,v in ids.items())
    diagnostics=Counter()
    calls=syscall_segments(events,diagnostics)
    kernel_ids=defaultdict(set)
    for _,lane,pid,_ in events:kernel_ids[lane].add(pid)
    assert sorted(kernel_ids)==list(range(4)) and all(len(v)==1 for v in kernel_ids.values())
    assert len(set.union(*kernel_ids.values()))==4
    bins=Counter();app=defaultdict(list);clock_error=0;details=[]
    for a in records(log,"ATTEMPT"):
        if a[-1]:app[a[0]%4].append((a[6],a[7],a[0]))
    intervals=[]
    for c in records(log,"PUBLICATION"):
        intervals.extend([(c[1],c[2],"prefix"),(c[3],c[4],"snapshot_write"),(c[4],c[5],"snapshot_sync"),(c[5],c[11],"finalize")])
    assert all(a<=b for a,b,_ in intervals) and all(u[1]<=v[0] for u,v in zip(intervals,intervals[1:]))
    ends=[s[1] for s in intervals]
    def book(phase,kind,length):
        bins[phase,kind]+=length
        categories[kind]+=length
        by_phase[phase]+=length
    def post(a,b,kind):
        i=bisect.bisect_right(ends,a)
        while i<len(intervals) and intervals[i][0]<b:
            start,end,phase=intervals[i]
            if a<start:book("outside",kind,start-a);a=start
            stop=min(end,b);book(phase,kind,stop-a);a=stop;i+=1
        book("outside",kind,b-a)
    for lane,rows in app.items():
        assert len(rows)==len(calls[lane]),(lane,len(rows),len(calls[lane]))
        for (a,b,req),parts in zip(rows,calls[lane]):
            categories=Counter();by_phase=Counter()
            start=parts[0][0]-origin;end=parts[-1][1]-origin
            error=max(0,a-start,end-b);clock_error=max(clock_error,error)
            assert error<=2000,(req,a,start,end,b,error)
            post(a,min(b,max(a,start)),"outside_syscall")
            for s,e,k in parts:
                left=max(a,min(b,s-origin));right=max(a,min(b,e-origin))
                if left<right:post(left,right,k)
            post(min(b,max(a,end)),b,"outside_syscall")
            assert sum(categories.values())==b-a==sum(by_phase.values())
            j=bisect.bisect_right(ends,a)
            phase=intervals[j][2] if j<len(intervals) and intervals[j][0]<=a else "outside"
            details.append(dict(request=req,lane=lane,namespace_tid=ids[lane][0],guest_tid=next(iter(kernel_ids[lane])),
                start_ns=a,end_ns=b,start_phase=phase,**{k:categories[k] for k in KINDS},**{"phase_"+p:by_phase[p] for p in PHASES}))
    assert sum(bins.values())==sum(b-a for rows in app.values() for a,b,_ in rows)
    return bins,clock_error,details,diagnostics

def parse_trial(folder,ops,cap,derived=None):
    p=folder/"app.log"
    log=p.read_text() if p.exists() else gzip.decompress(p.with_suffix(".log.gz").read_bytes()).decode()
    rep,mode,flag=folder.name.split("-");r,_,_=analyze(log,mode,int(rep),ops,cap)
    assert r["checkpoints"]==ops//cap-1 and r["coalesced"]==0
    r["observer"]=flag
    if flag=="plain":return r,[]
    name=re.search(r"OS_LANE,\d+,\d+,cw([0-9a-f]{10})\d",log)[1]
    check_metadata(folder)
    events=normalize(folder,name)
    bins,err,details,diagnostics=allocate(log,events)
    r["wake_while_scheduled"]=diagnostics["wake_while_scheduled"]
    if derived is not None:
        derived.mkdir(parents=True,exist_ok=True)
        with gzip.open(derived/(folder.name+".csv.gz"),"wt",newline="") as f:
            w=csv.DictWriter(f,details[0],lineterminator="\n");w.writeheader();w.writerows(details)
    r["clock_clip_ns"]=err;r["sync_s"]=sum(bins.values())/1e9
    for k in KINDS:r[k+"_s"]=sum(bins[p,k] for p in PHASES)/1e9
    return r,[dict(rep=rep,mode=mode,phase=p,kind=k,ns=bins[p,k]) for p in PHASES for k in KINDS]

def main():
    p=argparse.ArgumentParser();p.add_argument("raw",type=Path);p.add_argument("out",type=Path);a=p.parse_args()
    cfg=json.loads((a.raw/"environment.json").read_text());rows=[];cross=[]
    for folder in sorted(a.raw.iterdir()):
        if folder.is_dir():
            r,parts=parse_trial(folder,cfg["ops"],cfg["cap"],a.out/"matches");rows.append(r);cross+=parts
    pilot=cfg["ops"]==512
    assert {(r["rep"],r["mode"],r["observer"]) for r in rows}=={(i,m,f) for i in (range(1,3) if pilot else range(1,4)) for m in ("standard","fifo") for f in (("trace","plain") if pilot else ("trace",))}
    a.out.mkdir(exist_ok=True,parents=True)
    keys=sorted(set().union(*(r.keys() for r in rows)))
    save(a.out/"trials.csv",[{k:r.get(k,"") for k in keys} for r in rows]);save(a.out/"cross.csv",cross)
if __name__=="__main__":main()
