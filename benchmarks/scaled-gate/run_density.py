"""Matched sweep on identical seed-page fences. Fresh volume for each implementation/run."""
import argparse,json,hashlib,uuid
from pathlib import Path
from run import call,ROOT
def main():
    p=argparse.ArgumentParser();p.add_argument("out",type=Path);a=p.parse_args()
    root=a.out.resolve();root.mkdir(parents=True,exist_ok=False)
    image="catdb-pagepatch"
    (root/"environment.json").write_text(json.dumps(dict(image=call("docker","image","inspect",image,"--format","{{.Id}}"),commit=call("git","rev-parse","HEAD"),rows=1000000,cpus=2,memory_bytes=4294967296,hashes={str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in (ROOT/'benchmarks/scaled-gate').glob('*.rs')}),indent=2))
    for run in ("diagnostic",1,2,3):
        rep=1 if run=="diagnostic" else run
        for mode in (("arrivals","patch") if rep%2 else ("patch","arrivals")):
            name="catdb-density-"+uuid.uuid4().hex
            call("docker","volume","create",name)
            try:
                cmd=["docker","run","--rm","--network","none","--cpus","2","--memory","4g","--memory-swap","4g","-v",name+":/data","-e","SCALED_ROOT=/data","-e",f"SCALED_VARIANT={mode}","-e",f"DENSITY_REP={rep}","-e","DENSITY_COUNTS=1"]
                if run=="diagnostic":cmd+=["-e","CATDB_CHECKPOINT_PHASES=1"]
                print("START",run,mode,flush=True)
                log=call(*cmd,image,"cargo","test","--release","--locked","--lib","transactions::scaled_gate::checks::timing::density_compare","--","--ignored","--exact","--nocapture")
                (root/f"{run}-{mode}.log").write_text(log+"\n")
                assert len([l for l in log.splitlines() if l.startswith("DENSITY,")])==6
                print("DONE",run,mode,flush=True)
            finally:call("docker","volume","rm",name)
if __name__=="__main__":main()
