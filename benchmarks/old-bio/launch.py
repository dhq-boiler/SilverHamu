import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
import shutil

here=Path(__file__).resolve().parent
bench=Path(os.environ.get("CATDB_BENCH_ROOT",here.parent))
base=bench/"ext4-checkpoint-bytes"
sys.path.insert(0,str(base))
import run

def trial(out,rep,mode,trace,pilot):
    out.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(dir=out) as overlay:
        for src in (here/"observe.py",base/"layout.py"):
            shutil.copyfile(src,Path(overlay)/src.name)
        run.HERE=Path(overlay)
        run.one(out,rep,mode,trace,pilot,image="catdb-aged-load",range_writeback=True)

if __name__=="__main__":
    p=argparse.ArgumentParser()
    p.add_argument("out",type=Path);p.add_argument("--full",action="store_true")
    a=p.parse_args()
    modes=("standard","fifo","aged")
    if a.full:
        for i in range(3):
            for mode in modes[i:]+modes[:i]:
                folder=a.out/f"{i+1}-{mode}-trace"
                if not folder.exists():
                    trial(a.out,i+1,mode,True,False)
                missed=[s for s in json.loads((folder/"meta.json").read_text())["probe_profile"] if int(s.split()[-1])]
                if missed:
                    print("Incomplete capture:",folder.name,missed,flush=True)
                    continue
                from analyze_bio import audit
                audit(folder,16384,256)
    else:
        for i,order in enumerate(((False,True),(True,False)),1):
            for trace in order:trial(a.out,i,"aged",trace,True)
