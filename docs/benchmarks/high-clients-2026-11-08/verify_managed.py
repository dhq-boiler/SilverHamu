"""Verify the managed-WAL report against epoch samples, headers and expected recovered rows."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
from statistics import median


def check(condition, context):
    if not condition:
        raise ValueError(f"Managed WAL audit mismatch: {context}")


def expected_digest(keys, passes, size):
    result = hashlib.sha256()
    for index in range(keys):
        key = index.to_bytes(8, "big")
        pattern = hashlib.sha256(key + passes.to_bytes(8, "big")).digest()
        whole, remaining = divmod(size, len(pattern))
        result.update(key + pattern * whole + pattern[:remaining])
    return result.hexdigest()


def summarize(epochs):
    samples = sorted(ns for epoch in epochs for ns in epoch["latencies_ns"])
    elapsed = sum(epoch["wall_ns"] for epoch in epochs)
    count = len(samples)
    result = {"completed": count, "elapsed_s": elapsed / 1e9, "ops_s": count * 1e9 / elapsed,
              "max_ms": samples[-1] / 1e6}
    for p in (50, 95, 99):
        result[f"p{p}_ms"] = samples[(count * p + 99) // 100 - 1] / 1e6
    return result


def verify(directory, clients):
    report = json.loads((directory / "summary.json").read_text(encoding="utf-8"))
    for name, value in {"reps": 3, "keys": 2048, "passes": 5, "value_bytes": 2048, "epoch": 128,
                        "cpu_quota_per_container": 2, "memory_gib_per_container": 4}.items():
        check(report[name] == value, name)
    keys, passes, size, epoch_size = (report[k] for k in ("keys", "passes", "value_bytes", "epoch"))
    expected = expected_digest(keys, passes, size)
    schedule = [(stamp, start, min(keys, start + epoch_size)) for stamp in range(1, passes + 1)
                for start in range(0, keys, epoch_size)]
    types = ("all", "growth", "compaction", "none")
    summaries = {(count, stage): [] for count in clients for stage in types}
    counts = dict(completed_updates=0, epochs=0, growth_epochs=0, compaction_epochs=0,
                  reported_pinned_snapshot_checks=0, verified_reopens=0, recovered_rows_total=0)
    for rep in range(1, report["reps"] + 1):
        folder = directory / f"r{rep}"
        for count in clients:
            case = json.loads((folder / f"c{count}.json").read_text(encoding="utf-8"))
            identity = f"r{rep}/c{count}"
            check((case["clients"], case["rep"], case["server_image"], case["keys"], case["passes"],
                   case["value_bytes"]) == (count, rep, report["server_image"], keys, passes, size), identity)
            if count >= 32 or "max_connections" in case:
                check(case["max_connections"] == max(32, count + 2), (identity, "connections"))
            check(case["raw_latencies"] == f"c{count}-epochs.json.gz", identity)
            with gzip.open(folder / case["raw_latencies"], "rt", encoding="utf-8") as f:
                epochs = json.load(f)
            previous = case["initial"]
            check(previous == {"format": 4, "base": 0, "managed": True, "file_bytes": 1310720}, identity)
            groups = {stage: [] for stage in types}
            for row, (stamp, start, end) in zip(epochs, schedule, strict=True):
                context = identity, stamp, start
                check((row["pass"], row["first"], row["last"]) == (stamp, start, end), context)
                samples = row["latencies_ns"]
                check(len(samples) == end - start and all(type(ns) is int and 0 < ns <= row["wall_ns"]
                                                          for ns in samples), context)
                header = row["after"]
                check(row["before"] == previous and header["managed"] and header["format"] == 4, context)
                size_delta = header["file_bytes"] - previous["file_bytes"]
                base_delta = header["base"] - previous["base"]
                check(size_delta >= 0 and base_delta >= 0 and (size_delta == 0 or base_delta > 0), context)
                stage = "growth" if size_delta else "compaction" if base_delta else "none"
                check(row["change"] == stage, context)
                groups[stage].append(row)
                groups["all"].append(row)
                previous = header
            check(case["final"] == previous == case["reopen"]["header"], (identity, "reopened header"))
            for stage in ("growth", "compaction"):
                name = stage + "_epochs"
                check(case[name] == len(groups[stage]) >= 2, (identity, stage))
                counts[name] += len(groups[stage])
            for stage, measurements in groups.items():
                summary = summarize(measurements)
                check(summary == case["summary"][stage], (identity, stage, "trial statistics"))
                summaries[count, stage].append(summary)
            check(case["seed_snapshot_checks"] == passes + 1, (identity, "pinned reader"))
            reopen = case["reopen"]
            check(reopen["sha256"] == case["latest_sha256"] == expected, (identity, "row digest"))
            check(reopen["recovered_keys"] == keys and reopen["following_commit"] is True, (identity, "reopen"))
            counts["completed_updates"] += summaries[count, "all"][-1]["completed"]
            counts["epochs"] += len(epochs)
            counts["reported_pinned_snapshot_checks"] += case["seed_snapshot_checks"]
            counts["verified_reopens"] += 1
            counts["recovered_rows_total"] += reopen["recovered_keys"]
    seen = set()
    for group in report["groups"]:
        key = group["clients"], group["epoch_type"]
        check(key in summaries and key not in seen, key)
        seen.add(key)
        for name in ("ops_s", "p50_ms", "p95_ms", "p99_ms", "max_ms"):
            values = [row[name] for row in summaries[key]]
            check(group[name] == {"median": median(values), "min": min(values), "max": max(values)}, (key, name))
    check(seen == summaries.keys(), "aggregate group coverage")
    return dict(counts, clients=clients, expected_recovery_sha256=expected,
                medians=[dict(clients=count, ops_s=median(row["ops_s"] for row in summaries[count, "all"]),
                              p99_ms=median(row["p99_ms"] for row in summaries[count, "all"])) for count in clients])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--clients", nargs="+", type=int, default=[32, 64])
    args = parser.parse_args()
    print(json.dumps(verify(args.directory, args.clients), indent=2))
