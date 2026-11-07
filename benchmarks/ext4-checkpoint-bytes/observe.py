"""Bounded private tracefs observer. Thread events are scoped to the five workers and JBD2 events to the volume device."""
import os
from layout import bio_probe
import gzip
import json
from pathlib import Path
import re
import subprocess
import sys
import time

def main(tag):
    assert re.fullmatch("[0-9a-f]{10}",tag)
    mount=Path("/trace");control=Path("/control")
    subprocess.run(["mount","-t","tracefs","tracefs",str(mount)],check=True)
    instance=mount/"instances"/("catdb-"+tag)
    instance.mkdir()
    names=[f"cw{tag}{lane}" for lane in range(5)]
    def predicate(field):
        return "("+" || ".join(f'{field} == "{name}"' for name in names)+")"
    def put(f,v):
        (instance/f).write_text(str(v))
    filters={"syscalls/sys_enter_fsync":predicate("COMM"),"syscalls/sys_exit_fsync":predicate("COMM"),
             "sched/sched_switch":predicate("prev_comm")+" || "+predicate("next_comm"),
             "sched/sched_wakeup":predicate("comm")}
    device=os.stat("/subject").st_dev
    major,minor=os.major(device),os.minor(device)
    filters["ext4/ext4_sync_file_enter"]=predicate("COMM")
    for event in ("jbd2_start_commit","jbd2_end_commit"):
        filters["jbd2/"+event]=f"dev == {(major<<20)|minor}"
    group="catdb_"+tag
    registered=[]
    def define(cmd):
        fd=os.open(mount/"kprobe_events",os.O_WRONLY|os.O_APPEND)
        try:os.write(fd,(cmd+"\n").encode())
        finally:os.close(fd)
    enabled=[];trigger=False
    meta=dict(tag=tag,buffer_KiB_per_cpu=8192,clock="mono",filters=filters)
    try:
        put("tracing_on",0);put("trace_clock","mono");put("buffer_size_kb",8192)
        if (instance/"options/nsecs").exists():put("options/nsecs",1)
        meta.update(kernel=os.uname().release,device=f"{major},{minor}")
        for kind,symbol in (("target","ext4_fc_commit"),("waiting","jbd2_log_wait_commit")):
            name=kind+"_"+tag
            define(f"p:{group}/{name} {symbol} tx=$arg2:u32")
            registered.append(name)
            filters[group+"/"+name]=predicate("COMM")
        name="bio_"+tag
        define(f"p:{group}/{name} submit_bio "+bio_probe())
        registered.append(name)
        filters[group+"/"+name]=f"dev == {(major<<20)|minor} && bytes > 0 && endfn ~ 'ext4_end_bio*'"
        for syscall in ("write","pwrite64","writev","pwritev"):
            event="syscalls/sys_exit_"+syscall
            if (instance/"events"/event).exists():
                filters[event]=f'COMM == "{names[4]}"'
        for e,expr in filters.items():put(f"events/{e}/filter",expr)
        put("events/sched/sched_switch/trigger","stacktrace if "+predicate("prev_comm")+" && prev_state & 2")
        trigger=True
        for e in filters:put(f"events/{e}/enable",1);enabled.append(e)
        put("tracing_on",1)
        (control/"ready").write_text(tag)
        deadline=time.monotonic()+480
        while not (control/"stop").exists():
            if time.monotonic()>deadline:raise TimeoutError("bounded observation expired")
            time.sleep(.1)
    finally:
        try:
            put("tracing_on",0)
            meta["stats"]={p.parent.name:p.read_text() for p in (instance/"per_cpu").glob("cpu*/stats")}
            with (instance/"trace").open("rb") as source,gzip.open(control/"trace.gz","wb",compresslevel=1) as dest:
                while data:=source.read(1<<20):dest.write(data)
        finally:
            for e in enabled:put(f"events/{e}/enable",0)
            if trigger:put("events/sched/sched_switch/trigger","!stacktrace")
            meta["probe_profile"]=[line for line in (mount/"kprobe_profile").read_text().splitlines() if any(name in line for name in registered)]
            try:instance.rmdir()
            finally:
                for name in reversed(registered):define(f"-:{group}/{name}")
            meta["cleaned"]=True
            (control/"meta.json").write_text(json.dumps(meta,indent=2))
if __name__=="__main__":main(sys.argv[1])
