"""Reject a storage path unless sync/readback and cross-process exclusion work."""
import hashlib,json,os,pathlib,subprocess,uuid

def sync_readback(path,payload):
    created=False
    try:
        with path.open('xb') as f:
            created=True
            f.write(payload);f.flush();os.fsync(f.fileno())
        assert path.read_bytes()==payload
        return hashlib.sha256(payload).hexdigest()
    finally:
        if created:path.unlink()

def main():
    root=pathlib.Path('/data');control=pathlib.Path(os.environ['CATDB_BENCH_CONTROL'])
    digest=sync_readback(root/('probe-'+uuid.uuid4().hex),bytes(range(256))*16)
    child=subprocess.run(['silverhamu-server','--db','/data/catdb.wal','--bind','127.0.0.1:0','--token-file',str(control/'token')],capture_output=True,text=True,timeout=15)
    assert child.returncode!=0 and 'ownership lock unavailable' in child.stderr,(child.returncode,child.stdout,child.stderr)
    mounts=[x for x in pathlib.Path('/proc/self/mountinfo').read_text().splitlines() if x.split()[4] in ['/data','/control']]
    assert len(mounts)==2,mounts
    print(json.dumps({'sync_readback_sha256':digest,'exclusive_lock_refused':True,'lock_error':child.stderr.strip(),'mountinfo':mounts,'free_bytes':os.statvfs(root).f_bavail*os.statvfs(root).f_frsize}))
if __name__=='__main__':main()
