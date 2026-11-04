"""Compare Linux durable-write calls without changing a database format."""
import ctypes, hashlib, json, math, os, pathlib, random, statistics, time
root = pathlib.Path("/data/probe")
root.mkdir(exist_ok=False)
libc = ctypes.CDLL(None, use_errno=True)
libc.fallocate.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_longlong, ctypes.c_longlong]
cases = ["fsync", "fdatasync", "O_DSYNC", "O_DSYNC-reserved", "O_SYNC", "RWF_DSYNC"]
results = []
count = 512
prefix = 32 * 1024 * 1024
for rep in range(1, 4):
    order = cases[:]
    random.Random(20261105+rep).shuffle(order)
    for name in order:
        path = root / f"{name}-{rep}"
        fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
        digest = hashlib.sha256()
        seed = b"s" * (1024 * 1024)
        for _ in range(32):
            assert os.write(fd, seed) == len(seed)
            digest.update(seed)
        if name.endswith("reserved"):
            assert libc.fallocate(fd, 1, prefix, 1024*1024) == 0
        os.fsync(fd)
        os.close(fd)
        flags = os.O_RDWR
        if name.startswith("O_DSYNC"): flags |= os.O_DSYNC
        if name == "O_SYNC": flags |= os.O_SYNC
        fd = os.open(path, flags)
        os.lseek(fd, prefix, os.SEEK_SET)
        samples = []
        for sequence in range(count):
            payload = sequence.to_bytes(8,"little") + b"x"*592
            start = time.perf_counter_ns()
            if name == "RWF_DSYNC":
                assert os.pwritev(fd, [payload], prefix+sequence*600, os.RWF_DSYNC) == 600
            else:
                assert os.write(fd, payload) == 600
                if name == "fsync": os.fsync(fd)
                if name == "fdatasync": os.fdatasync(fd)
            samples.append(time.perf_counter_ns()-start)
            digest.update(payload)
        os.close(fd)
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        assert actual == digest.hexdigest()
        ordered = sorted(samples)
        results.append({"case":name,"rep":rep,"durable_mean_ms":statistics.mean(samples)/1e6,
                        "durable_p99_ms":ordered[math.ceil(count*.99)-1]/1e6,"durable_ns":samples,"sha256":actual})
        path.unlink()
print(json.dumps({"kernel":os.uname().release,"iterations":count,"prefix_bytes":prefix,"batch_bytes":600,"cases":results}))
