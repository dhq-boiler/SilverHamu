"""Summarize paired prewrite pilots and attribute foreground and worker page writes."""
import csv
import re
from pathlib import Path
import argparse

def save(path, rows):
    with path.open("w", newline="") as f:
        writer = csv.DictWriter(f, rows[0], lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)

def checkpoints(text):
    return [tuple(map(int, m)) for m in re.findall(r"PREWRITE_CHECKPOINT,(\d+),(\d+),(\d+)", text)]

def main():
    parser = argparse.ArgumentParser()
    for arg in ("pilot", "mixed", "out"):
        parser.add_argument(arg, type=Path)
    args = parser.parse_args()
    rows=[]
    last=None
    for line in args.pilot.read_text().splitlines():
        cp=checkpoints(line)
        if cp: last=cp[-1]
        if line.startswith("PREWRITE_PILOT,"):
            _,rep,hot,on,staged,extra,prep,final,total=line.split(",")
            assert last and last[0]==32
            rows.append(dict(rep=int(rep),placement="same" if hot=="true" else "other",
                             prewrite=on,prepared=int(staged),reused=last[1],
                             early_bytes=int(extra),final_bytes=last[2],total_bytes=int(extra)+last[2],
                             early_ms=int(prep)/1e6,final_ms=int(final)/1e6,phase_sum_ms=int(total)/1e6))
    assert len(rows)==12
    args.out.mkdir(exist_ok=True,parents=True)
    save(args.out/"pilot.csv",rows)
    runs=[]
    for f in sorted(args.mixed.glob("*.log")):
        txt=f.read_text()
        cp=[c for c in checkpoints(txt) if c[0]>0]
        assert [c[0] for c in cp]==[256,512,768]
        matches=re.findall(r"PREWRITE_TOTAL,(\d+),(\d+)",txt)
        assert len(matches)==int("prewrite" in f.name)
        prepared,early=map(int,matches[0]) if matches else (0,0)
        final=sum(c[2] for c in cp)
        runs.append(dict(run=f.stem,prepared=prepared,reused=sum(c[1] for c in cp),
                         early_bytes=early,final_bytes=final,total_bytes=early+final))
    assert len(runs)==6
    save(args.out/"write-volume.csv",runs)

if __name__ == "__main__":
    main()
