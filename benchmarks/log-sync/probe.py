"""Disposable Linux I/O probe, not an engine throughput benchmark."""
import ctypes, hashlib, json, math, os, pathlib, random, statistics, time

root = pathlib.Path("/data/probe")
root.mkdir(exist_ok=False)
libc = ctypes.CDLL(None, use_errno=True)
libc.fallocate.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_longlong, ctypes.c_longlong]
libc.fallocate.restype = ctypes.c_int
cases = [
    ("append-fsync-split", False, False, False, False),
    ("append-fdatasync-split", True, False, False, False),
    ("append-fsync-reserved", False, True, False, False),
    ("append-fdatasync-reserved", True, True, False, False),
    ("append-fsync-combined", False, False, False, True),
    ("append-fdatasync-combined", True, False, False, True),
    ("fixed-fsync", False, False, True, True),
    ("fixed-fdatasync", True, False, True, True),
]
count = 512
prefix = 32 * 1024 * 1024
results = []
for rep in range(1, 4):
    order = cases[:]
    random.Random(2605 + rep).shuffle(order)
    for name, data_only, reserve, fixed, combined in order:
        path = root / f"{name}-{rep}"
        fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
        digest = hashlib.sha256()
        seed = b"s" * (1024 * 1024)
        for _ in range(32):
            assert os.write(fd, seed) == len(seed)
            digest.update(seed)
        if fixed:
            assert os.write(fd, bytes(count * 600)) == count * 600
        if reserve:
            assert libc.fallocate(fd, 1, prefix, 1024 * 1024) == 0, ctypes.get_errno()
            assert os.fstat(fd).st_size == prefix
        os.fsync(fd)
        os.lseek(fd, prefix, os.SEEK_SET)
        samples = []
        writes = []
        for sequence in range(count):
            payload = sequence.to_bytes(8, "little") + b"x" * 592
            start = time.perf_counter_ns()
            if combined:
                assert os.write(fd, payload) == len(payload)
            else:
                assert os.write(fd, payload[:300]) == 300
                assert os.write(fd, payload[300:]) == 300
            before_sync = time.perf_counter_ns()
            (os.fdatasync if data_only else os.fsync)(fd)
            end = time.perf_counter_ns()
            samples.append(end - before_sync)
            writes.append(before_sync - start)
            digest.update(payload)
        os.close(fd)
        actual = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                actual.update(chunk)
        assert actual.digest() == digest.digest()
        ordered = sorted(samples)
        results.append({"case": name, "rep": rep, "sync_mean_ms": statistics.mean(samples)/1e6,
                        "sync_p99_ms": ordered[math.ceil(count*.99)-1]/1e6,
                        "write_mean_ms": statistics.mean(writes)/1e6,
                        "sync_ns": samples, "write_ns": writes, "sha256": actual.hexdigest()})
        path.unlink()
print(json.dumps({"kernel": os.uname().release, "prefix_bytes": prefix, "batch_bytes": 600,
                  "iterations": count, "cases": results}))
