from pathlib import Path
import csv,json,sys
from statistics import mean,median
def load_case(path,run):
    lines=(path/(run['label']+'.log')).read_text().splitlines()
    requests={int(v[2]):list(map(int,v[3:])) for l in lines if l.startswith("SCALED_REQUEST,") for v in [l.split(",")]}
    phases={int(v[1]):list(map(int,v[2:])) for l in lines if l.startswith("COMMIT_PHASE,") for v in [l.split(",")]}
    assert len(requests)==len(phases)==1024
    rows=[]
    for i,(lane,seq,start,end,retries) in requests.items():
        attempts,*ns=phases[i]
        assert attempts==retries+1 and sum(ns)<=end-start and len(ns)==5
        rows.append(dict(mode=run["mode"],id=i,lane=lane,attempts=attempts,request_ms=(end-start)/1e6,
                         fifo_ms=ns[0]/1e6,prepare_ms=ns[1]/1e6,write_ms=ns[2]/1e6,sync_ms=ns[3]/1e6,publish_ms=ns[4]/1e6,
                         outside_ms=(end-start-sum(ns))/1e6))
    return rows
if __name__=='__main__':
    import argparse
    p=argparse.ArgumentParser();p.add_argument('raw',type=Path);p.add_argument('out',type=Path);a=p.parse_args()
    rows=[v for r in json.loads((a.raw/'runs.json').read_text()) for v in load_case(a.raw,r)]
    with a.out.open('w',newline='') as f:
        w=csv.DictWriter(f,rows[0],lineterminator='\n');w.writeheader();w.writerows(rows)
    for mode in ('combine','logreuse'):
        g=[r for r in rows if r["mode"]==mode]
        print(mode,{k:round(mean(r[k] for r in g),4) for k in rows[0] if k.endswith('_ms')})
