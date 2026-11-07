"""Copy only measurements used in the analysis; scrub unrelated guest task identities."""
import gzip,json,re,sys
from pathlib import Path
from partition import LINE

def export_kernel(source,out,tag):
    wanted={f"cw{tag}{i}" for i in range(4)}
    switch=re.compile(r'prev_comm=(.*?) prev_pid=(\d+).*?prev_state=(\S+) ==> next_comm=(.*?) next_pid=(\d+)')
    keep=False
    with gzip.open(source,"rt") as f,gzip.open(out,"wt",compresslevel=6) as g:
        for line in f:
            if line.startswith(" => "):
                if keep:
                    symbol=line[4:].strip().split()[0]
                    if re.fullmatch(r"[A-Za-z0-9_.]+",symbol):g.write(" => "+symbol+"\n")
                continue
            keep=False
            m=LINE.match(line)
            if not m:continue
            comm,pid,cpu,sec,frac,body=m.groups();comm=comm.strip()
            if body.startswith("sched_switch:"):
                x=switch.search(body);old,i,state,new,j=x.groups()
                assert old in wanted or new in wanted
                if old not in wanted:old,i="other","0"
                if new not in wanted:new,j="other","0"
                body=f"sched_switch: prev_comm={old} prev_pid={i} prev_state={state} ==> next_comm={new} next_pid={j}"
            elif body.startswith("sched_wakeup:"):
                x=re.search(r'comm=(.*?) pid=(\d+)',body);assert x[1] in wanted
                body=f"sched_wakeup: comm={x[1]} pid={x[2]}"
            elif body=="<stack trace>":
                if comm not in wanted:continue
                keep=True
            elif body.startswith("sys_fsync(fd:"):
                assert comm in wanted
                body="sys_fsync(fd: 0)"
            elif body.startswith("sys_fsync ->"):
                assert comm in wanted
            else:continue
            if comm not in wanted:comm,pid="other","0"
            g.write(f"{comm}-{pid} [{cpu}] ..... {sec}.{frac}: {body}\n")

def export(raw,dest):
    dest.mkdir(parents=True)
    (dest/"environment.json").write_bytes((raw/"environment.json").read_bytes())
    for d in sorted(raw.iterdir()):
        if not d.is_dir():continue
        o=dest/d.name;o.mkdir()
        (o/"app.log.gz").write_bytes(gzip.compress((d/"app.log").read_bytes(),mtime=0))
        if (d/"meta.json").exists():
            meta=(d/"meta.json").read_bytes();(o/"meta.json").write_bytes(meta)
            export_kernel(d/"trace.gz",o/"trace.gz",json.loads(meta)["tag"])

if __name__=="__main__":export(Path(sys.argv[1]),Path(sys.argv[2]))
