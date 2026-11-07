"""Cross-check published per-epoch evidence without accessing the guest kernel."""
import csv
import gzip
import math
import sys
from pathlib import Path
from statistics import median
from xml.etree.ElementTree import parse

def read(path):
    with (gzip.open(path,"rt",newline="") if path.suffix==".gz" else path.open(newline="")) as f:
        return list(csv.DictReader(f))

def verify(root):
    full=read(root/"trials.csv")
    pubs=read(root/"publications.csv")
    groups=read(root/"groups.csv")
    assert len(full)==6 and len(pubs)==378 and len(read(root/"pilot-trials.csv"))==8
    for r in full:
        mode,rep=r["mode"],r["rep"]
        where=lambda x:(x["mode"],x["rep"])==(mode,rep)
        p=[x for x in pubs if where(x)]
        assert sorted(int(x["publication"]) for x in p)==list(range(1,64))
        assert all(x["app_B"]==x["write_returns_B"] for x in p)
        assert sum(int(x["bio_B"]) for x in p)==int(r["pub_bio_B"])
        epoch=read(root/f"{rep}-{mode}-trace-epochs.csv.gz")
        assert len(set(x["tx"] for x in epoch))==len(epoch)==int(r["epochs"])
        assert sum(int(x["writers"]) for x in epoch)==int(r["wal_resolved"])
        for period in ("prior","during","first5"):
            for g in (g for g in groups if where(g) and g["window"]==period):
                values=[]
                for x in epoch:
                    if period=="first5" and x["remaining_at_5ms_us"]=="":continue
                    same,other,unknown=(int(x[period+"_"+c+"_B"]) for c in ("same","other","unknown"))
                    label=("mixed/same/unknown" if same+unknown else "zero" if not other else
                           "other <1 MiB" if other<2**20 else "other >=1 MiB")
                    if label==g["exposure"]:values.append(float(x["remaining_at_5ms_us" if period=="first5" else "duration_us"])/1000)
                assert len(values)==int(g["n"]) and math.isclose(median(values),float(g["median_ms"]))
    for fig in ("dose.svg","performance.svg"):
        assert parse(root/fig).getroot().tag.endswith("svg")
    print("Checked all 378 publication totals, epoch identities, grouped counts and durations, and both SVG files.")

if __name__=="__main__": verify(Path(sys.argv[1]))
