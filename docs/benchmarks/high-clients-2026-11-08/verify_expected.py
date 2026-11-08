"""Replay the seeded durable-update schedule without reading the driver's expected rows."""
import argparse
import hashlib
import json
import random
from pathlib import Path


def state_digest(stamps):
    digest = hashlib.sha256()
    for index, stamp in enumerate(stamps):
        key = index.to_bytes(8, "big")
        digest.update(key + key + stamp.to_bytes(8, "big") + b"x" * 240)
    return digest.hexdigest()


def replay(rows, operations, repetition, clients, initial_hash):
    order = clients.copy()
    random.Random(912 + repetition).shuffle(order)
    stamps = [0] * rows
    changed = 0
    stages = {"initial": (initial_hash, changed)}
    for count in order:
        for worker in range(count):
            rng = random.Random(424242 + 100 * repetition + worker)
            partition = range(worker, rows, count)
            first_stamp = repetition * 1_000_000_000 + count * 10_000_000 + worker * operations + 1
            for sequence in range(operations // count):
                row = rng.choice(partition)
                if stamps[row] == 0:
                    changed += 1
                stamps[row] = first_stamp + sequence
        stages[f"update-{count}"] = (state_digest(stamps), changed)
    stages["reopen"] = stages[f"update-{order[-1]}"]
    return stages


def require(condition, context):
    if not condition:
        raise ValueError(f"Audit mismatch: {context}")


def verify(directory, clients, with_append_baseline=False):
    environment = json.loads((directory / "environment.json").read_text(encoding="utf-8"))
    engines = {"catdb-service", "postgres", "oracle"}
    if with_append_baseline:
        engines.add("catdb-baseline")
    require(set(environment["engines"]) == engines, "engines")
    require(environment["cases"] == [["update", count] for count in clients], "case order")
    settings = {"rows": 100_000, "value_bytes": 256, "ops_per_case": 8_000, "reps": 3,
                "redo_log_bytes": 64 * 1024**2, "oracle_initrans": 16,
                "single_request": True, "serialization_retries": 3}
    for key, value in settings.items():
        require(environment[key] == value, key)

    rows = environment["rows"]
    operations = environment["ops_per_case"]
    require(all(0 < n <= rows and operations % n == 0 for n in clients), "worker partitions")
    initial_hash = state_digest([0] * rows)
    schedules = {rep: replay(rows, operations, rep, clients, initial_hash)
                 for rep in range(1, environment["reps"] + 1)}
    seen = set()
    checked = 0
    for run in environment["runs"]:
        engine, rep = run["engine"], run["rep"]
        identity = engine, rep
        require(engine in engines and rep in schedules and identity not in seen, identity)
        seen.add(identity)
        require(run["status"] == "passed", identity)
        if engine.startswith("catdb-"):
            require(run["wal_format"] == (4 if engine == "catdb-service" else 2), (identity, "WAL"))
        elif engine == "postgres":
            for setting, expected in {"fsync": "on", "synchronous_commit": "on",
                                      "transaction_isolation": "serializable"}.items():
                require(run["metadata"][f"show {setting}"] == [[expected]], (identity, setting))
        elif engine == "oracle":
            require(run["metadata"]["SELECT ini_trans FROM user_tables WHERE table_name='KV'"] == [[16]],
                    (identity, "INITRANS"))
        checks = run["verification"]
        require([check["stage"] for check in checks] == list(schedules[rep]), (identity, "stages"))
        for check in checks:
            stage = check["stage"]
            digest, changed = schedules[rep][stage]
            require(check["rows"] == rows and check["empty_and_last"] and check["updated_keys"] == changed,
                    (identity, stage, "rows"))
            require(check["sha256"] == check["expected_sha256"] == digest, (identity, stage, "SHA-256"))
            checked += 1
    require(seen == {(engine, rep) for engine in engines for rep in schedules}, "complete runs")
    return {"verified_seeded_content_hashes": checked, "verified_reopens": len(seen),
            "rows_per_check": rows, "clients": clients, "engines": sorted(engines)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--clients", nargs="+", type=int, default=[4, 16, 32, 64])
    parser.add_argument("--with-append-baseline", action="store_true")
    args = parser.parse_args()
    print(json.dumps(verify(args.directory, args.clients, args.with_append_baseline), indent=2))
