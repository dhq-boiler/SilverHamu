"""Run the real WAL with four ungated writers and one checkpoint worker."""
import argparse
import json
import subprocess
import uuid
from pathlib import Path
from run import call,ROOT

def main():
    p=argparse.ArgumentParser();p.add_argument("out",type=Path);p.add_argument("--pilot",action="store_true")
    a=p.parse_args();a.out.mkdir(parents=True,exist_ok=False)
    n,ops,cap=(10000,512,32) if a.pilot else (1000000,16384,256)
    trials=[]
    config=dict(rows=n,ops=ops,cap=cap,image=call("docker","image","inspect","catdb-independent","--format","{{.Id}}"),cpus=2,memory=2**32)
    (a.out/"environment.json").write_text(json.dumps(config,indent=2))
    for rep in range(1,2 if a.pilot else 4):
        for mode in (("off","on") if rep%2 else ("on","off")):
            volume="catdb-independent-"+uuid.uuid4().hex
            call("docker","volume","create",volume)
            try:
                cmd=["docker","run","--rm","--network","none","--cpus","2","--memory","4g","--memory-swap","4g","-v",volume+":/data"]
                for key,val in dict(SCALED_ROOT="/data",SCALED_VARIANT="prefix-"+mode,SCALED_ROWS=n,SCALED_OPS=ops,SCALED_CAP=cap).items():
                    cmd+=["-e",f"{key}={val}"]
                cmd+=["catdb-independent","cargo","test","--release","--locked","--lib","transactions::scaled_gate::checks::timing::independent_campaign","--","--ignored","--exact","--nocapture"]
                print("START",rep,mode,flush=True)
                with (a.out/f"{rep}-{mode}.log").open("w") as f:
                    subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT,check=True,cwd=ROOT,timeout=600)
                assert f"INDEPENDENT_OK,{ops}" in (a.out/f"{rep}-{mode}.log").read_text()
                trials.append(dict(rep=rep,mode=mode));(a.out/"runs.json").write_text(json.dumps(trials,indent=2))
                print("DONE",rep,mode,flush=True)
            finally:call("docker","volume","rm",volume)
if __name__=="__main__":main()
