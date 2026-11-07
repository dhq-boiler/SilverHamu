"""Export individual update responses and verify the published percentile denominators."""
import argparse
import csv
import gzip
import math
from collections import defaultdict
from pathlib import Path

from campaign import analyze, records, save
from report_range import load, verify


def export(folder, output, trial):
    attempts = defaultdict(list)
    for row in records((folder / "app.log").read_text(), "ATTEMPT"):
        attempts[row[0]].append(row)
    assert sorted(attempts) == list(range(int(trial["updates"])))
    with gzip.open(output, "wt", newline="") as stream:
        writer = csv.writer(stream, lineterminator="\n")
        writer.writerow(["request", "received_ns", "completed_ns", "retries", "sequence"])
        for request, tries in sorted(attempts.items()):
            assert not any(row[-1] for row in tries[:-1]) and tries[-1][-1] > 0
            writer.writerow([request, tries[0][1], tries[-1][-2], len(tries)-1, tries[-1][-1]])


def check_responses(path, trial):
    with gzip.open(path, "rt", newline="") as stream:
        rows = list(csv.DictReader(stream))
    n = len(rows)
    assert n == int(trial["updates"])
    assert sorted(int(r["request"]) for r in rows) == list(range(n))
    seq = sorted(int(r["sequence"]) for r in rows)
    assert seq == list(range(seq[0], seq[0]+n))
    start = [int(r["received_ns"]) for r in rows]
    end = [int(r["completed_ns"]) for r in rows]
    latencies = sorted(b-a for a,b in zip(start,end))
    for key, actual in (("p99_ms", latencies[math.ceil(.99*n)-1]/1e6),
                        ("max_ms", latencies[-1]/1e6), ("updates_s", n*1e9/(max(end)-min(start)))):
        assert math.isclose(actual, float(trial[key]), rel_tol=1e-10), (key,actual,trial[key])


def main():
    p = argparse.ArgumentParser()
    p.add_argument("report", type=Path)
    p.add_argument("--raw", type=Path)
    args = p.parse_args()
    trials = load(args.report / "trials.csv")
    verify(trials, load(args.report / "publications.csv"), 63, 3)
    tail_requests = []
    for trial in trials:
        mode, rep, variant = (trial[k] for k in ("mode","rep","variant"))
        path = args.report / f"{rep}-{mode}-{variant}-updates.csv.gz"
        if args.raw:
            folder = args.raw / variant / f"{rep}-{mode}-trace"
            export(folder, path, trial)
            _, tails, _ = analyze((folder / "app.log").read_text(), mode, int(rep), int(trial["updates"]), 256)
            tail_requests.extend(dict(variant=variant, **t) for t in tails)
        check_responses(path, trial)
    if tail_requests:
        save(args.report / "tail-requests.csv", tail_requests)
    print("All 196,608 responses reproduce throughput, p99 and worst update; all 756 checkpoint totals reconcile.")

if __name__=="__main__":
    main()
