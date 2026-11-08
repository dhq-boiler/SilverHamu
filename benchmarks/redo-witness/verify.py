"""Recreate the update schedule independently and verify service results before plotting."""
import argparse
import gzip
import hashlib
import json
import math
from pathlib import Path
import random
from statistics import median


def expected_hashes(rows, operations, repetitions):
    hashes = {}
    for rep in range(1, repetitions + 1):
        clients = [4, 16]
        random.Random(912 + rep).shuffle(clients)
        stamps = {}

        def digest():
            result = hashlib.sha256()
            for index in range(rows):
                key = index.to_bytes(8, "big")
                result.update(key + key + stamps.get(index, 0).to_bytes(8, "big") + b"x" * 240)
            return result.hexdigest()

        hashes[rep, "initial"] = digest()
        for count in clients:
            for worker in range(count):
                rng = random.Random(424242 + rep * 100 + worker)
                for index in range(operations // count):
                    key = rng.randrange((rows - worker + count - 1) // count) * count + worker
                    stamps[key] = rep * 1_000_000_000 + count * 10_000_000 + worker * operations + index + 1
            hashes[rep, f"update-{count}"] = digest()
        hashes[rep, "reopen"] = digest()
    return hashes


def verify(path, crash_result=None):
    folder = Path(path)
    env = json.loads((folder / "environment.json").read_text())
    engines = ["catdb-baseline", "catdb-service", "postgres", "oracle"]
    assert set(env["engines"]) == set(engines)
    assert env["redo_log_bytes"] == 64 * 1024**2 and env["oracle_initrans"] == 16
    assert env["single_request"] and env["serialization_retries"] == 3
    assert (env["rows"], env["value_bytes"], env["ops_per_case"], env["reps"]) == (100000, 256, 8000, 3)
    if crash_result:
        crash = json.loads(Path(crash_result).read_text())
        assert crash["status"] == "passed" and crash["initial_format"] == 4
        assert crash["image"] == env["images"]["catdb-service"]
    expected = expected_hashes(env["rows"], env["ops_per_case"], env["reps"])
    verified = set()
    for run in env["runs"]:
        engine = run["engine"]
        assert run["status"] == "passed"
        if engine.startswith("catdb-"):
            assert run["wal_format"] == (4 if engine == "catdb-service" else 2)
        elif engine == "oracle":
            assert run["metadata"]["SELECT ini_trans FROM user_tables WHERE table_name='KV'"] == [[16]]
        elif engine == "postgres":
            assert run["metadata"]["show fsync"] == run["metadata"]["show synchronous_commit"] == [["on"]]
        for check in run["verification"]:
            identity = (engine, run["rep"], check["stage"])
            assert identity not in verified
            assert check["rows"] == env["rows"] and check["empty_and_last"]
            assert check["sha256"] == check["expected_sha256"] == expected[run["rep"], check["stage"]]
            verified.add(identity)
    assert verified == {(engine, rep, stage) for engine in engines for rep, stage in expected}
    stats = {}
    records = json.loads((folder / "results.json").read_text())
    for record in records:
        identity = (record["engine"], record["rep"], record["clients"])
        assert identity not in stats and record["workload"] == "update" and not record["errors"]
        assert record["retries"] == len(record["retry_events"])
        with gzip.open(folder / record["latency_file"], "rt") as fp:
            latencies = sorted(json.load(fp))
        assert len(latencies) == record["completed"] == record["attempted"] == 8000
        for percentile in (50, 95, 99):
            sample = latencies[math.ceil(len(latencies) * percentile / 100) - 1]
            assert record[f"p{percentile}_ms"] == sample / 1e6
        assert math.isclose(record["ops_s"], len(latencies) / record["elapsed_s"], rel_tol=1e-12)
        stats[identity] = record
    assert len(records) == 24 and len(env["runs"]) == 12
    workloads = []
    for clients in (4, 16):
        summary = {}
        for engine in engines:
            cases = [stats[engine, rep, clients] for rep in (1, 2, 3)]
            rates = [case["ops_s"] for case in cases]
            tails = [case["p99_ms"] for case in cases]
            summary[engine] = dict(throughput=median(rates), p99_ms=median(tails),
                                   trials_ops=rates, trials_p99_ms=tails, retries=sum(c["retries"] for c in cases))
        rates = summary["catdb-service"]["trials_ops"]
        summary["wins_vs_each"] = {
            engine: sum(a > b for a, b in zip(rates, summary[engine]["trials_ops"]))
            for engine in engines if engine != "catdb-service"
        }
        workloads.append({"clients": clients, "engines": summary})
    audit = dict(checked_latencies=len(records) * 8000, verified_seeded_content_hashes=len(verified),
                 verified_reopens=len(env["runs"]), workloads=workloads)
    (folder / "independent-audit.json").write_text(json.dumps(audit, indent=2) + "\n")
    return audit


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("--crash-result", type=Path)
    args = parser.parse_args()
    print(json.dumps(verify(args.directory, args.crash_result), indent=2))
