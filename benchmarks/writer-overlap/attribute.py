"""Replay WAL fsync windows against target-inode ext4 submissions and lock acquisitions."""
import argparse
from bisect import bisect_left
from collections import defaultdict
import csv
import gzip
from itertools import accumulate
import os
from pathlib import Path
import sys

BENCH = Path(os.environ.get("CATDB_BENCH_ROOT", Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(BENCH / "ext4-checkpoint-bytes"))
from join import run, records


def volume(times, prefix, start, end):
    return prefix[bisect_left(times, end)] - prefix[bisect_left(times, start)]


def replay(folder, out):
    rep, mode, _ = folder.name.split("-")
    events = []
    pubs, _, _ = run(folder, lambda i, t, b: events.append((t, b)))
    times = [t for t, _ in events]
    assert times == sorted(times)
    prefix = list(accumulate((b for _, b in events), initial=0))
    assert len(pubs) == 63 and prefix[-1] == sum(p["bio_B"] for p in pubs)
    log = (folder / "app.log").read_text()
    anchors = records(log, "OS_CLOCK")
    offset = (max(a[1] for a in anchors) + min(a[2] for a in anchors)) // 2
    holds = sorted(records(log, "HOLD"), key=lambda h: h[2])
    predecessors = {h[2]: prev[0] for prev, h in zip(holds, holds[1:])}
    lanes = {n: iter([h for h in holds if h[0] == n]) for n in range(4)}
    attempts = records(log, "ATTEMPT")
    fields = ("request", "generation", "lane", "previous_lane", "prior_B", "during_B", "sync_ns")
    counts = defaultdict(lambda: [0, 0])
    generations = []
    with gzip.open(out / f"{rep}-{mode}.csv.gz", "wt", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(fields)
        for a in attempts:
            lane = a[0] % 4
            h = next(lanes[lane])
            assert a[3] <= h[1] <= h[2] <= h[3] <= h[4] <= a[9]
            if not a[-1]:
                continue
            assert h[2] <= a[6] <= a[7] <= h[3]
            generations.append(a[-1])
            start, end = offset + a[6], offset + a[7]
            prior = volume(times, prefix, start - 10_000_000, start)
            during = volume(times, prefix, start, end)
            previous = predecessors.get(h[2], -1)
            writer.writerow((a[0], a[-1], lane, previous, prior, during, end-start))
            group = counts[bool(prior), bool(during)]
            group[0] += 1
            group[1] += end-start
    assert all(next(it, None) is None for it in lanes.values())
    assert len(generations) == 16384 and sorted(generations) == list(range(min(generations), min(generations)+16384))
    print(rep, mode, sorted(counts.items()), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("raw", type=Path)
    parser.add_argument("out", type=Path)
    args = parser.parse_args()
    args.out.mkdir(exist_ok=True)
    for folder in sorted(args.raw.glob("*-trace")):
        replay(folder, args.out)
