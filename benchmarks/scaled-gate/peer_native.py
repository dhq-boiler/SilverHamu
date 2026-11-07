"""Run the Linux test-only storage policies on the shared peer update key stream."""
import argparse
import json
import math
import random
import hashlib
import tempfile
import uuid
from pathlib import Path
from run import call


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("out",type=Path)
    args=parser.parse_args()
    args.out.mkdir(parents=True,exist_ok=False)
    results=[]
    image="catdb-append-reuse"
    image_id=call("docker","image","inspect",image,"--format","{{.Id}}")
    for rep in (1,2,3):
        with tempfile.TemporaryDirectory() as tmp:
            trace=Path(tmp)/"keys.txt"
            keys=[]
            for lane in range(4):
                rng=random.Random(424242+rep*100+lane)
                keys.extend(rng.randrange(25000)*4+lane for _ in range(256))
            trace.write_text("\n".join(map(str,keys))+"\n")
            stamps={}
            for index,key_id in enumerate(keys):
                lane,sequence=divmod(index,256)
                stamps[key_id]=rep*1000000000+40000000+lane*1024+sequence+1
            digest=hashlib.sha256()
            for key_id in range(100000):
                k=key_id.to_bytes(8,'big')
                digest.update(k+k+stamps.get(key_id,0).to_bytes(8,'big')+b'x'*240)
            for mode in (("combine","logreuse") if rep%2 else ("logreuse","combine")):
                volume="catdb-peer-native-"+uuid.uuid4().hex
                call("docker","volume","create",volume)
                try:
                    output=call("docker","run","--rm","--network","none","--cpus","2","--memory","4g","--memory-swap","4g","-v",volume+":/data","-v",tmp+":/trace:ro","-e","SCALED_ROOT=/data","-e",f"SCALED_VARIANT={mode}","-e",f"PEER_REP={rep}",image,"cargo","test","--release","--locked","--lib","transactions::scaled_gate::checks::timing::peer_updates","--","--ignored","--exact","--nocapture")
                finally:call("docker","volume","rm",volume)
                (args.out/f"{rep}-{mode}.log").write_text(output+"\n",encoding="utf8")
                summary=next(l.split("PEER_SUMMARY,")[1] for l in output.splitlines() if "PEER_SUMMARY," in l)
                trial,elapsed,verified=map(int,summary.split(","))
                samples=sorted(int(l.split(",")[1]) for l in output.splitlines() if l.startswith("PEER_NS,"))
                assert trial==rep and verified==100000 and len(samples)==1024
                results.append(dict(engine=mode,rep=rep,ops_s=1024e9/elapsed,p99_ms=samples[math.ceil(.99*1024)-1]/1e6,max_ms=samples[-1]/1e6,rows_verified=verified,expected_sha256=digest.hexdigest(),elapsed_ns=elapsed,image=image_id))
                (args.out/"results.json").write_text(json.dumps(results,indent=2)+"\n")
                print(mode,rep,results[-1]["ops_s"],flush=True)
if __name__=="__main__":main()
