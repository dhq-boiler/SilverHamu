"""Run on fresh volumes; the separate observer is the only container with mount capability."""
import argparse,json,subprocess,time,uuid
from pathlib import Path
HERE=Path(__file__).resolve().parent
def call(*cmd,timeout=600):
    r=subprocess.run(list(map(str,cmd)),capture_output=True,text=True,timeout=timeout)
    if r.returncode:raise RuntimeError(r.stderr[-2000:]+r.stdout[-1000:])
    return r.stdout.strip()
def one(out,rep,mode,trace,pilot):
    tag=uuid.uuid4().hex[:10]
    name=f"{rep}-{mode}-"+("trace" if trace else "plain");folder=out/name;folder.mkdir()
    vol="catdb-os-"+tag;obs=vol+"-observer"
    n,ops,cap=(10000,512,32) if pilot else (1_000_000,16384,256)
    call("docker","volume","create",vol)
    owned=False
    try:
        if trace:
            call("docker","run","-d","--name",obs,"--read-only","--network","none","--cap-drop","ALL",
                 "--cap-add","SYS_ADMIN","--security-opt","no-new-privileges","--tmpfs","/trace:rw,nosuid,nodev,mode=700",
                 "-v",str(folder.resolve())+":/control","-v",str((HERE/"observe.py").resolve())+":/observe.py:ro",
                 "catdb-fifo-products","python","/observe.py",tag)
            owned=True
            deadline=time.monotonic()+20
            while not (folder/"ready").exists():
                if time.monotonic()>deadline or call("docker","inspect","--format","{{.State.Running}}",obs)=="false":
                    raise RuntimeError(call("docker","logs",obs))
                time.sleep(.2)
        cmd=["docker","run","--rm","--name",vol+"-db","--network","none","--cpus","2","--memory","4g","--memory-swap","4g","-v",vol+":/data"]
        for k,v in dict(SCALED_ROOT="/data",SCALED_VARIANT="prefix-on",SCALED_ROWS=n,SCALED_OPS=ops,SCALED_CAP=cap,EXACT_PUBLICATIONS=1,SYNC_OS_TAG=tag).items():
            cmd+=["-e",f"{k}={v}"]
        cmd+=["catdb-sync-partition","cargo","test","--locked","--release"]
        if mode=="fifo":cmd+=["--features","fifo-writer"]
        cmd+=["--lib","transactions::scaled_gate::checks::timing::independent_campaign","--","--ignored","--exact","--nocapture"]
        print("START",name,flush=True)
        with (folder/"app.log").open("w") as f:subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT,check=True,timeout=450)
        assert f"INDEPENDENT_OK,{ops}" in (folder/"app.log").read_text()
    finally:
        try:
            if owned:
                (folder/"stop").touch()
                try:
                    if call("docker","wait",obs,timeout=90)!="0":
                        r=subprocess.run(["docker","logs",obs],capture_output=True,text=True)
                        raise RuntimeError(r.stdout+r.stderr)
                finally:call("docker","rm","-f",obs)
        finally:
            subprocess.run(["docker","rm","-f",vol+"-db"],capture_output=True)
            call("docker","volume","rm",vol)
    print("DONE",name,flush=True)
def main():
    a=argparse.ArgumentParser();a.add_argument("out",type=Path);a.add_argument("--pilot",action="store_true");args=a.parse_args()
    out=args.out.resolve();out.mkdir(parents=True,exist_ok=False)
    (out/"environment.json").write_text(json.dumps(dict(rows=10000 if args.pilot else 1000000,ops=512 if args.pilot else 16384,
        cap=32 if args.pilot else 256,db_image=call("docker","image","inspect","--format","{{.Id}}","catdb-sync-partition"),
        observer_image=call("docker","image","inspect","--format","{{.Id}}","catdb-fifo-products"),cpu=2,memory=2**32),indent=2))
    for rep in range(1,3 if args.pilot else 4):
        for mode in (("standard","fifo") if rep%2 else ("fifo","standard")):
            for trace in (((True,False) if rep%2 else (False,True)) if args.pilot else (True,)):
                one(out,rep,mode,trace,args.pilot)
if __name__=="__main__":main()
