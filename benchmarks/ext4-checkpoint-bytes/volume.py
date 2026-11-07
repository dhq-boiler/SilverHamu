import csv,gzip
from collections import defaultdict
from statistics import median,mean
from pathlib import Path

def amounts(out):
    buckets=defaultdict(list)
    for file in sorted(out.glob("*-trace-epochs.csv.gz")):
        rep,mode,*_=file.name.split("-")
        with gzip.open(file,"rt") as stream:
            for r in csv.DictReader(stream):
                for window in ("prior","during","first5"):
                    if not r[window+"_same_B"] or int(r[window+"_same_B"])+int(r[window+"_unknown_B"]):continue
                    b=int(r[window+"_other_B"])
                    label="zero" if not b else "0-8 MiB" if b<8*2**20 else "8-32 MiB" if b<32*2**20 else "32+ MiB"
                    ms=float(r["remaining_at_5ms_us" if window=="first5" else "duration_us"])/1000
                    buckets[mode,rep,window,label].append((b,ms))
    with (out/"volume.csv").open("w",newline="") as f:
        w=csv.writer(f,lineterminator="\n");w.writerow(["mode","rep","window","exposure","n","median_MiB","median_ms"])
        for k,v in sorted(buckets.items()):
            w.writerow([*k,len(v),median(b/2**20 for b,_ in v),median(d for _,d in v)])

if __name__=="__main__":
    import sys
    amounts(Path(sys.argv[1]))
