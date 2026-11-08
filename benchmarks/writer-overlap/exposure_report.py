"""Report matched workloads, fixed-window exposure and observed overlap."""
import argparse
import csv
import gzip
from pathlib import Path
from statistics import mean

MODES = ("standard", "fifo", "aged")


def groups(rows, field):
    return [[r for r in rows if bool(r[field]) == exposed] for exposed in (False, True)]


def split(a, b):
    # The symmetric weights give the same decomposition with either lock as reference.
    a0, a1 = groups(a, "prior_B")
    b0, b1 = groups(b, "prior_B")
    time = lambda rows: mean(r["sync_ns"] for r in rows)
    frequency = (len(b1)-len(a1)) * (time(a1)-time(a0)+time(b1)-time(b0))/2e9
    elapsed = (sum(r["sync_ns"] for r in b)-sum(r["sync_ns"] for r in a))/1e9
    return frequency, elapsed-frequency


def report(out, trials):
    with trials.open() as f:
        trial = {(int(r["rep"]), r["mode"]): r for r in csv.DictReader(f) if r["case"] == "continuous"}
    data = {}
    table = []
    for rep in (1, 2, 3):
        for mode in MODES:
            with gzip.open(out / f"{rep}-{mode}.csv.gz", "rt") as f:
                rows = [{k:int(v) for k,v in row.items()} for row in csv.DictReader(f)]
            data[rep,mode] = rows
            assert sorted(r["request"] for r in rows) == list(range(16384))
            total = sum(r["sync_ns"] for r in rows)/1e9
            run = trial[rep,mode]
            assert abs(total-float(run["wal_s"])) < 1e-6
            neither = [r for r in rows if not (r["during_B"] or r["prior_B"])]
            previous_cp = [r for r in rows if r["previous_lane"]==4]
            table.append([rep,mode,round(float(run["updates_s"]),2),round(float(run["sent_p99_ms"]),2),
                round(float(run["max_sent_ms"]),2),round(total,3),sum(bool(r["prior_B"]) for r in rows),
                sum(bool(r["during_B"]) for r in rows),len(neither),round(sum(r["sync_ns"] for r in neither)/1e9,3),
                len(previous_cp),sum(r["previous_lane"]==r["lane"] for r in rows),
                sum(bool(r["prior_B"]) for r in previous_cp),
                round(mean(r["prior_B"] for r in rows)/1048576,3),
                round(mean(r["sync_ns"] for r in rows if r["prior_B"])/1e6,3),
                round(mean(r["sync_ns"] for r in rows if not r["prior_B"])/1e6,3)])
    headers = "Pair,Lock,Updates/s,P99 ms,Max ms,WAL s,Prior exposed,During exposed,Neither,Neither WAL s,After checkpoint,Same lane,Prior exposed after checkpoint,Mean prior MiB,Mean sync prior exposed ms,Mean sync prior clear ms".split(",")
    with (out/"summary.csv").open("w",newline="") as f:
        w = csv.writer(f); w.writerow(headers); w.writerows(table)
    print(*table,sep="\n")
    print("Frequency and within-group differences in seconds:")
    for rep in (1,2,3):
        print(rep,split(data[rep,"standard"],data[rep,"aged"]))
    svg = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 860 465" role="img"><title>WAL sync time by checkpoint exposure</title><rect width="860" height="465" fill="white"/><g font-family="system-ui,sans-serif" fill="#18344c">',
           '<text x="18" y="30" font-size="23">Where writers spent time in WAL sync</text>',
           '<text x="18" y="55" font-size="14">aged = Aged writer priority (16 ms). Exposure covers submissions in the prior 10 ms or during sync.</text>',
           '<rect x="166" y="70" width="15" height="15" fill="#396586"/><text x="188" y="83">No observed submission</text><rect x="450" y="70" width="15" height="15" fill="#bb7938"/><text x="472" y="83">Exposed</text><text x="730" y="83">Total (s)</text>']
    for i, row in enumerate(table):
        rep,mode=row[:2];total=row[5]; quiet=row[9]; y=110+37*i
        svg += [f'<text x="18" y="{y+4}">{rep} · {mode}</text>',
            f'<rect x="166" y="{y-11}" width="{quiet*3.35}" height="19" fill="#396586"/>',
            f'<rect x="{166+quiet*3.35}" y="{y-11}" width="{(total-quiet)*3.35}" height="19" fill="#bb7938"/>',
            f'<text x="730" y="{y+4}">{total:.2f}</text>']
    svg += ['<text x="18" y="450" font-size="13">Target-inode submissions from the ext4 guest. Concurrent device work can continue after submission.</text></g></svg>']
    (out/"overlap.svg").write_text("".join(svg),encoding="utf8")

if __name__ == "__main__":
    p=argparse.ArgumentParser()
    p.add_argument("out",type=Path);p.add_argument("trials",type=Path)
    args=p.parse_args();report(args.out,args.trials)
