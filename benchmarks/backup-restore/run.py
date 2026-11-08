"""Measure the catdb archive CLI and a synchronized OS file copy on the same closed database."""

import argparse
import hashlib
import itertools
import json
import platform
import shutil
import statistics
import struct
import subprocess
import tempfile
import time
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def run(*args):
    return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE).stdout.strip()


def duration_ms(*args):
    start = time.perf_counter_ns()
    run(*args)
    return (time.perf_counter_ns() - start) / 1e6


def trial(cli, fixture, profile, source, expected_hash, method, target, compare_archives):
    artifact = target / "archive"
    restored = target / "restored.ndb"
    if method != "copy":
        backup_ms = duration_ms(cli, "backup", source, artifact)
        restore_ms = duration_ms(cli, "restore", artifact, restored)
    else:
        backup_ms = duration_ms(fixture, "copy", source, artifact)
        restore_ms = duration_ms(fixture, "copy", artifact, restored)
    if compare_archives and sha256(restored) != expected_hash:
        raise RuntimeError(f"{profile} {method}: restored bytes differ before opening")
    first_ms = int(run(fixture, "first-read", restored, profile)) / 1e6
    if not compare_archives and sha256(restored) != expected_hash:
        raise RuntimeError(f"{profile} {method}: restored database differs from its source")
    run(fixture, "verify", restored, profile)
    result = {"method": method, "backup_ms": backup_ms, "restore_ms": restore_ms,
            "first_read_ms": first_ms, "restore_and_first_read_ms": restore_ms + first_ms,
            "archive_bytes": artifact.stat().st_size}
    if compare_archives and method != "copy":
        result["archive_sha256"] = sha256(artifact)
    return result


def measure(methods, fixture, baseline_fixture, folder, reps, names):
    compare_archives = "previous" in methods
    orders = list(itertools.permutations(methods))
    profiles = {}
    for name in names:
        directory = folder / name
        directory.mkdir()
        source = directory / "source.ndb"
        run(baseline_fixture, "seed", source, name)
        with source.open("rb") as f:
            header = f.read(48)
        if struct.unpack_from("<I", header, 8)[0] != 4:
            raise RuntimeError("expected a format-4 fixture")
        original_hash = sha256(source)
        samples = []
        for rep in range(0 if compare_archives else 1, reps + 1):
            order = orders[(rep - 1) % len(orders)] if rep else orders[0]
            for method in order:
                with tempfile.TemporaryDirectory(dir=directory) as tmp:
                    engine = fixture if method == "catdb" else baseline_fixture
                    sample = trial(methods[method], engine, name, source, original_hash,
                                   method, Path(tmp), compare_archives)
                    if rep:
                        samples.append(dict(rep=rep, **sample))
        if sha256(source) != original_hash:
            raise RuntimeError("the source database changed")
        if compare_archives and len({s["archive_sha256"] for s in samples if s["method"] != "copy"}) != 1:
            raise RuntimeError(f"{name}: old and new encoders produced different v1 archives")
        medians = {}
        ranges = {}
        for method in methods:
            runs = [sample for sample in samples if sample["method"] == method]
            medians[method], ranges[method] = {}, {}
            for metric in ("backup_ms", "restore_ms", "first_read_ms", "restore_and_first_read_ms", "archive_bytes"):
                values = [r[metric] for r in runs]
                medians[method][metric] = statistics.median(values)
                ranges[method][metric] = [min(values), max(values)]
        profiles[name] = {"source_bytes": source.stat().st_size, "source_sha256": original_hash,
                          "samples": samples, "medians": medians, "ranges": ranges}
        print(name, json.dumps(medians), flush=True)
    return profiles


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catdb", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--baseline-fixture", type=Path, help="fixture built with the reference engine for plain copies")
    parser.add_argument("--baseline-catdb", type=Path, help="also compare the previous v1 archive encoder")
    parser.add_argument("--profiles", nargs="+", choices=("spare", "dense", "near-full"), default=("spare", "dense"))
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--reps", type=int, default=6)
    args = parser.parse_args()
    if args.reps < 3:
        parser.error("use three or more repetitions")
    if args.baseline_catdb and args.reps % 6:
        parser.error("use a multiple of six repetitions to balance the three methods")
    if len(set(args.profiles)) != len(args.profiles):
        parser.error("each profile may be requested once")
    if args.out.exists():
        parser.error("output file already exists")
    cli, fixture = args.catdb.resolve(strict=True), args.fixture.resolve(strict=True)
    baseline_fixture = args.baseline_fixture.resolve(strict=True) if args.baseline_fixture else fixture
    methods = {"copy": baseline_fixture}
    if args.baseline_catdb:
        methods["previous"] = args.baseline_catdb.resolve(strict=True)
    methods["catdb"] = cli
    args.work_dir.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(args.work_dir).free < 1024**3:
        parser.error("at least 1 GiB of free space is needed")
    with tempfile.TemporaryDirectory(prefix="catdb-backup-study-", dir=args.work_dir) as tmp:
        profiles = measure(methods, fixture, baseline_fixture, Path(tmp), args.reps, args.profiles)
    result = {"system": platform.system(), "os_version": platform.version(), "machine": platform.machine(),
              "catdb_sha256": sha256(cli), "fixture_sha256": sha256(fixture),
              "baseline_fixture_sha256": sha256(baseline_fixture),
              "repetitions_per_method": args.reps, "profiles": profiles}
    if args.baseline_catdb:
        result["baseline_catdb_sha256"] = sha256(methods["previous"])
        result["warmups_per_method"] = 1
    with args.out.open("x", encoding="utf-8") as output:
        json.dump(result, output, indent=2)
        output.write("\n")


if __name__ == "__main__":
    main()
