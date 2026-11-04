"""Validate latency summaries and full-content equality across paired variants."""
import gzip
import itertools
import json
import math
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
rows = json.loads((root / "results.json").read_text(encoding="utf-8"))
env = json.loads((root / "environment.json").read_text(encoding="utf-8"))
variants = set(env["variants"])
assert env["reps"] == 3
conditions = set(itertools.product(env.get("workloads", ["get", "range100", "update"]), env.get("clients", [1, 4])))
expected = set(itertools.product(variants, range(1, 4), conditions))
seen = set()
for row in rows:
    identity = (row["variant"], row["rep"], (row["workload"], row["clients"]))
    assert row["engine"] == "catdb-service" and row["rows"] == 100000 and row["value_bytes"] == 256
    assert identity not in seen
    seen.add(identity)
    with gzip.open(root / row["latencies_ns_file"], "rt", encoding="utf-8") as stream:
        values = sorted(json.load(stream))
    assert len(values) == row["completed"] == row["attempted"] == env["ops"] == 8000
    assert all(isinstance(v, int) and v > 0 for v in values)
    assert not row["errors"] and row["retries"] == 0
    for p in (50, 95, 99):
        assert row[f"p{p}_ms"] == values[math.ceil(len(values) * p / 100) - 1] / 1e6
    assert row["max_ms"] == values[-1] / 1e6
    assert abs(row["ops_s"] - len(values) / row["elapsed_s"]) < 1e-8
assert seen == expected
assert len(env["runs"]) == len(variants) * 3
stages = {"initial", "reopen"} | {f"{w}-{c}" for w, c in conditions}
for rep in range(1, 4):
    runs = [r for r in env["runs"] if r["rep"] == rep]
    assert {r["variant"] for r in runs} == variants and len(runs) == len(variants)
    for run in runs:
        assert run["status"] == "passed"
        assert len(run["checks"]) == len(stages)
        assert {c["stage"] for c in run["checks"]} == stages
        for check in run["checks"]:
            assert check["rows"] == 100000 and check["empty_and_last"]
            assert check["sha256"] == check["expected_sha256"]
    for stage in stages:
        assert len({c["sha256"] for r in runs for c in r["checks"] if c["stage"] == stage}) == 1
print(f"Validated {len(rows)} cases, {sum(r['completed'] for r in rows)} operations, "
      f"{len(env['runs']) * len(stages)} full-content checks; paired and restart equality.")
