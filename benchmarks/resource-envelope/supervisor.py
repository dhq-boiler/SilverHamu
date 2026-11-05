"""Bounded cgroup-v2 observer; its own cost belongs to the server cgroup."""
import json, os, pathlib, signal, subprocess, time

ROOT = pathlib.Path('/sys/fs/cgroup')
OUT = pathlib.Path('/data')

def read(name):
    return (ROOT / name).read_text().strip()

def pairs(name):
    return {k: int(v) for k, v in (line.split() for line in read(name).splitlines())}

def snapshot(pid):
    status = pathlib.Path(f'/proc/{pid}/status').read_text() if pathlib.Path(f'/proc/{pid}').exists() else ''
    rss = next((int(x.split()[1])*1024 for x in status.splitlines() if x.startswith('VmRSS:')), 0)
    return {'time_ns': time.monotonic_ns(), 'memory': int(read('memory.current')),
            'peak': int(read('memory.peak')), 'swap': int(read('memory.swap.current')),
            'stat': pairs('memory.stat'), 'events': pairs('memory.events'),
            'cpu': pairs('cpu.stat'), 'rss': rss}

def main():
    sampled = os.environ.get('SAMPLE', '1') == '1'
    tag = os.environ.get('PHASE', 'boot')
    limits = {x: read(x) for x in ['cpu.max', 'memory.max', 'memory.swap.max']}
    (OUT / f'{tag}-limits.json').write_text(json.dumps(limits))
    with (OUT / f'{tag}-server.log').open('w') as log:
        p = subprocess.Popen(['catdb-server','--db','/data/catdb.wal','--bind','0.0.0.0:7400',
            '--allow-remote','--token-file','/data/token','--transaction-timeout-secs','3600'], stdout=log, stderr=log)
        signal.signal(signal.SIGTERM, lambda *_: p.send_signal(signal.SIGTERM) if p.poll() is None else None)
        signal.signal(signal.SIGINT, lambda *_: p.send_signal(signal.SIGINT) if p.poll() is None else None)
        with (OUT / f'{tag}-resources.jsonl').open('w', buffering=1) as f:
            start = time.monotonic(); count = 0; ready = False
            while p.poll() is None:
                if not ready and 'catdb-server ready' in (OUT/f'{tag}-server.log').read_text():
                    (OUT/f'{tag}-ready.json').write_text(json.dumps({'process_ready_ms':(time.monotonic()-start)*1000,'poll_resolution_ms':100 if sampled else 500}))
                    ready = True
                if sampled or count == 0:
                    f.write(json.dumps(snapshot(p.pid), separators=(',', ':'))+'\n')
                count += 1
                if time.monotonic()-start > 900:
                    p.kill(); break
                time.sleep(.1 if sampled else .5)
            p.wait()
            f.write(json.dumps(snapshot(p.pid), separators=(',', ':'))+'\n')
        (OUT/f'{tag}-exit.json').write_text(json.dumps({'returncode':p.returncode}))
        raise SystemExit(0 if p.returncode == 0 else 1)

if __name__ == '__main__': main()
