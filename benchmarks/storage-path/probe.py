"""Reject a storage path unless sync/readback and cross-process exclusion work."""
import hashlib,json,os,pathlib,subprocess,uuid
root=pathlib.Path('/data');control=pathlib.Path(os.environ['CATDB_BENCH_CONTROL'])
p=root/('probe-'+uuid.uuid4().hex);payload=bytes(range(256))*16
try:
    with p.open('xb') as f:
        f.write(payload);f.flush();os.fsync(f.fileno())
    assert p.read_bytes()==payload
finally:
    if p.exists():p.unlink()
child=subprocess.run(['catdb-server','--db','/data/catdb.wal','--bind','127.0.0.1:0','--token-file',str(control/'token')],capture_output=True,text=True,timeout=15)
assert child.returncode!=0 and 'ownership lock unavailable' in child.stderr,(child.returncode,child.stdout,child.stderr)
mounts=[x for x in pathlib.Path('/proc/self/mountinfo').read_text().splitlines() if x.split()[4] in ['/data','/control']]
assert len(mounts)==2,mounts
print(json.dumps({'sync_readback_sha256':hashlib.sha256(payload).hexdigest(),'exclusive_lock_refused':True,'lock_error':child.stderr.strip(),'mountinfo':mounts,'free_bytes':os.statvfs(root).f_bavail*os.statvfs(root).f_frsize}))
