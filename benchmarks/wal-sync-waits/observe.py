"""Bounded private tracefs observer. Only events for the four named writers are enabled."""
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
    names=[f"cw{tag}{lane}" for lane in range(4)]
    def predicate(field):
        return "("+" || ".join(f'{field} == "{name}"' for name in names)+")"
    def put(f,v):
        (instance/f).write_text(str(v))
    filters={"syscalls/sys_enter_fsync":predicate("COMM"),"syscalls/sys_exit_fsync":predicate("COMM"),
             "sched/sched_switch":predicate("prev_comm")+" || "+predicate("next_comm"),
             "sched/sched_wakeup":predicate("comm")}
    enabled=[];trigger=False
    meta=dict(tag=tag,buffer_KiB_per_cpu=8192,clock="mono",filters=filters)
    try:
        put("tracing_on",0);put("trace_clock","mono");put("buffer_size_kb",8192)
        if (instance/"options/nsecs").exists():put("options/nsecs",1)
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
            instance.rmdir()
            meta["cleaned"]=True
            (control/"meta.json").write_text(json.dumps(meta,indent=2))
if __name__=="__main__":main(sys.argv[1])
