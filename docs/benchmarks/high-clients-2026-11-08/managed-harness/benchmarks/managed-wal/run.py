"""Exercise managed WAL growth, compaction, pinned readers and crash recovery on the release service."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
import gzip
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import secrets
import statistics
import struct
import subprocess
import tempfile
import time
import uuid


def check(client, **request):
    response = client.request(**request)
    if not response.get("ok"):
        raise RuntimeError(str(response))
    return response


def record_value(key, stamp, size):
    block = hashlib.sha256(struct.pack(">QQ", key, stamp)).digest()
    return (block * ((size + len(block) - 1) // len(block)))[:size]


def key_hex(key):
    return key.to_bytes(8, "big").hex()


def scan(client):
    found = {}
    cursor = None
    while True:
        request = {"op": "scan", "start": "", "rows": 128, "bytes": 262144, "visits": 65536}
        if cursor is not None:
            request["after"] = cursor
        page = check(client, **request)
        for key, value in page["rows"]:
            assert key not in found
            found[key] = value
        cursor = page["next_after"]
        if cursor is None:
            return found


def final_state(keys, passes, size):
    return {key_hex(k): record_value(k, passes, size).hex() for k in range(keys)}


def digest(rows):
    h = hashlib.sha256()
    for key in sorted(rows):
        h.update(bytes.fromhex(key))
        h.update(bytes.fromhex(rows[key]))
    return h.hexdigest()


def header():
    path = Path("/data/state.ndb")
    with path.open("rb") as file:
        raw = file.read(48)
        size = os.fstat(file.fileno()).st_size
    version = struct.unpack_from("<I", raw, 8)[0]
    base, chunk_field = struct.unpack_from("<QI", raw, 32)
    return {"format": version, "base": base, "managed": bool(chunk_field & (1 << 31)), "file_bytes": size}


def latency_summary(samples, wall_ns):
    ordered = sorted(samples)
    def percentile(fraction):
        return ordered[math.ceil(len(ordered) * fraction) - 1] / 1e6
    return {"completed": len(samples), "elapsed_s": wall_ns / 1e9, "ops_s": len(samples) * 1e9 / wall_ns,
            "p50_ms": percentile(.50), "p95_ms": percentile(.95), "p99_ms": percentile(.99), "max_ms": ordered[-1] / 1e6}


def workload(connect, args):
    original = header()
    assert original["format"] == 4 and original["managed"], original
    seed_keys = 32
    seed = {key_hex(k): record_value(k, 0, args.value_bytes).hex() for k in range(seed_keys)}
    with connect() as setup:
        check(setup, op="begin")
        for key, value in seed.items():
            check(setup, op="put", key=key, value=value)
        initial_version = int(check(setup, op="commit")["version"])

    epochs, versions = [], set()
    state = original
    with ExitStack() as stack:
        pinned = stack.enter_context(connect())
        check(pinned, op="begin_read")
        assert scan(pinned) == seed
        clients = [stack.enter_context(connect()) for _ in range(args.clients)]
        pool = stack.enter_context(ThreadPoolExecutor(max_workers=args.clients))
        for stamp in range(1, args.passes + 1):
            for first in range(0, args.keys, args.epoch):
                last = min(first + args.epoch, args.keys)
                def write(lane):
                    samples, committed = [], []
                    for k in range(first + lane, last, args.clients):
                        started = time.perf_counter_ns()
                        value = record_value(k, stamp, args.value_bytes)
                        reply = check(clients[lane], op="put_commit", key=key_hex(k), value=value.hex())
                        samples.append(time.perf_counter_ns() - started)
                        committed.append(int(reply["version"]))
                    return samples, committed
                start = time.perf_counter_ns()
                results = list(pool.map(write, range(args.clients)))
                elapsed = time.perf_counter_ns() - start
                samples = [v for times, _ in results for v in times]
                for _, committed in results:
                    versions.update(committed)
                after = header()
                assert after["managed"] and after["format"] == 4 and after["base"] >= state["base"]
                assert after["file_bytes"] >= state["file_bytes"]
                change = "none"
                if after["file_bytes"] > state["file_bytes"]:
                    assert after["base"] > state["base"]
                    change = "growth"
                elif after["base"] > state["base"]:
                    change = "compaction"
                epochs.append({"pass": stamp, "first": first, "last": last, "wall_ns": elapsed, "latencies_ns": samples,
                               "change": change, "before": state, "after": after})
                state = after
            assert scan(pinned) == seed, "old reader no longer sees its seed snapshot"
        check(pinned, op="rollback")
    count = args.keys * args.passes
    assert len(versions) == count and min(versions) == initial_version + 1 and max(versions) == initial_version + count
    expected = final_state(args.keys, args.passes, args.value_bytes)
    with connect() as reader:
        check(reader, op="begin_read")
        assert scan(reader) == expected
        check(reader, op="rollback")
    groups = {"all": epochs}
    for change in ("growth", "compaction", "none"):
        groups[change] = [e for e in epochs if e["change"] == change]
    assert len(groups["growth"]) >= 2, "fixture did not observe repeated growth"
    assert len(groups["compaction"]) >= 2, "fixture did not observe repeated compaction"
    summary = {k: latency_summary([ns for e in v for ns in e["latencies_ns"]], sum(e["wall_ns"] for e in v))
               for k, v in groups.items() if v}
    return {"clients": args.clients, "keys": args.keys, "passes": args.passes, "value_bytes": args.value_bytes,
            "initial": original, "final": state, "seed_snapshot_checks": args.passes+1,
            "latest_sha256": digest(expected), "growth_epochs": len(groups["growth"]),
            "compaction_epochs": len(groups["compaction"]), "summary": summary, "epochs": epochs}


def after_restart(connect, args):
    expected = final_state(args.keys, args.passes, args.value_bytes)
    state = header()
    assert state["format"] == 4 and state["managed"]
    with connect() as client:
        check(client, op="begin_read")
        rows = scan(client)
        assert rows == expected
        check(client, op="rollback")
        assert check(client, op="get_latest", key="ff")["value"] is None
        check(client, op="put_commit", key="ff", value="aa")
        assert check(client, op="get_latest", key="ff")["value"] == "aa"
    return {"header":state, "recovered_keys":len(rows), "sha256":digest(rows), "following_commit":True}


def docker(*args, env=None, allow_failure=False):
    result = subprocess.run(["docker", *args], env=env, capture_output=True, text=True, encoding="utf-8", timeout=600)
    if result.returncode and not allow_failure:
        raise RuntimeError(result.stderr[-3000:])
    return result.stdout.strip()


def fixture(args, count, image_id, default_cmd, folder):
    repo = Path(__file__).resolve().parents[2]
    name = "catdb-managed-" + uuid.uuid4().hex[:14]
    volume, network, server, client = (name + s for s in ("-data", "-net", "-server", "-client"))
    token = secrets.token_hex(32)
    with tempfile.TemporaryDirectory() as tmp, ExitStack() as clean:
        secret = Path(tmp) / "token"
        secret.write_text(token)
        secret.chmod(0o444)
        docker("network", "create", "--internal", network)
        clean.callback(docker,"network","rm",network,allow_failure=True)
        docker("volume","create",volume)
        clean.callback(docker,"volume","rm",volume,allow_failure=True)
        limits=["--network",network,"--cpus","2","--memory","4g"]
        # The pinned reader and the image's periodic healthcheck each need a connection.
        connection_limit = max(32, count + 2)
        docker("run","-d","--name",server,*limits,"--network-alias","database","--read-only","--cap-drop","ALL",
               "--security-opt","no-new-privileges:true","-v",volume+":/data",
               "-v",str(secret)+":/run/secrets/catdb_token:ro",args.image,
               *default_cmd,"--max-connections",str(connection_limit))
        clean.callback(docker,"rm","-f",server,allow_failure=True)
        docker("run","-d","--name",client,*limits,"--read-only","-v",volume+":/data:ro",
               "-v",str(repo/"scripts")+":/scripts:ro","-v",str(Path(__file__).resolve().parent)+":/study:ro",
               "-e","BENCH_TOKEN",args.client_image,"sleep","infinity",env=dict(os.environ,BENCH_TOKEN=token))
        clean.callback(docker,"rm","-f",client,allow_failure=True)
        def run_phase(phase):
            return json.loads(docker("exec",client,"python","-B","/study/run.py","--phase",phase,
                "--clients",str(count),"--keys",str(args.keys),"--passes",str(args.passes),
                "--epoch",str(args.epoch),"--value-bytes",str(args.value_bytes)))
        result = run_phase("exercise")
        result["server_image"] = image_id
        result["max_connections"] = connection_limit
        raw=folder / f"c{count}-epochs.json.gz"
        with gzip.open(raw,"wt",encoding="utf-8") as out:
            json.dump(result.pop("epochs"),out)
        result["raw_latencies"] = raw.name
        docker("kill","--signal","KILL",server)
        assert docker("inspect","--format","{{.State.ExitCode}}",server) == "137"
        docker("start",server)
        result["reopen"] = run_phase("reopen")
        assert result["reopen"]["sha256"] == result["latest_sha256"]
        return result


def main():
    p=argparse.ArgumentParser()
    p.add_argument("--out",type=Path)
    p.add_argument("--image",default="catdb:local")
    p.add_argument("--client-image",default="python:3.12-slim-bookworm")
    p.add_argument("--clients",type=int,nargs="+",default=[4,16])
    p.add_argument("--reps",type=int,default=1)
    p.add_argument("--keys",type=int,default=2048)
    p.add_argument("--passes",type=int,default=5)
    p.add_argument("--epoch",type=int,default=128)
    p.add_argument("--value-bytes",type=int,default=2048)
    p.add_argument("--phase",choices=["exercise","reopen"],help=argparse.SUPPRESS)
    args=p.parse_args()
    if args.reps < 1 or min(args.clients) < 1 or len(set(args.clients)) != len(args.clients) or args.epoch<max(args.clients) or args.keys<32 or args.passes<1 or not 1 <= args.value_bytes <= 65536:
        p.error("invalid workload dimensions")
    if max(args.clients) + 2 > 256:
        p.error("at most 254 writers fit alongside the pinned reader and healthcheck")
    if args.phase:
        assert len(args.clients)==1
        args.clients=args.clients[0]
        spec=importlib.util.spec_from_file_location("catdb_client","/scripts/catdb-client.py")
        mod=importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        def connect():
            return mod.Client("database",7400,os.environ["BENCH_TOKEN"])
        deadline=time.monotonic()+60
        while True:
            try:
                with connect() as client:check(client,op="ping")
                break
            except (OSError, ConnectionError):
                if time.monotonic() >= deadline:raise
                time.sleep(.1)
        result = workload(connect,args) if args.phase=="exercise" else after_restart(connect,args)
        print(json.dumps(result))
        return
    if not args.out:p.error("--out is required")
    args.out.mkdir(parents=True,exist_ok=False)
    image=docker("image","inspect",args.image,"--format","{{.Id}}")
    default_cmd = json.loads(docker("image", "inspect", args.image, "--format", "{{json .Config.Cmd}}"))
    cases = []
    for rep in range(1, args.reps+1):
        folder = args.out if args.reps == 1 else args.out / f"r{rep}"
        folder.mkdir(exist_ok=True)
        for count in args.clients if rep % 2 else reversed(args.clients):
            result = fixture(args, count, image, default_cmd, folder)
            result["rep"] = rep
            (folder/f"c{count}.json").write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
            cases.append(result)
            print(f"r{rep} c{count}: {result['growth_epochs']} growth, {result['compaction_epochs']} compaction epochs, "
                  f"{result['summary']['all']['ops_s']:.1f}/s; recovered {result['reopen']['recovered_keys']} keys",flush=True)
    aggregate = {"server_image": image, "reps": args.reps, "keys": args.keys, "passes": args.passes,
                 "value_bytes": args.value_bytes, "epoch": args.epoch, "cpu_quota_per_container": 2,
                 "memory_gib_per_container": 4, "groups": []}
    for count in args.clients:
        rows = [r for r in cases if r["clients"] == count]
        assert len({r["latest_sha256"] for r in rows}) == 1
        for stage in ("all", "none", "growth", "compaction"):
            times = [r["summary"][stage] for r in rows]
            group = {"clients": count, "epoch_type": stage}
            for field in ("ops_s","p50_ms","p95_ms","p99_ms","max_ms"):
                values = [trial[field] for trial in times]
                group[field] = {"median":statistics.median(values), "min":min(values), "max":max(values)}
            aggregate["groups"].append(group)
    (args.out/"summary.json").write_text(json.dumps(aggregate,indent=2)+"\n",encoding="utf-8")

if __name__=="__main__":
    main()
