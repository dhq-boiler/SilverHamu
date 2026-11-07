"""Exercise the shipped Linux cdylib and service executable in a disposable container."""
import ctypes as C
import os
import secrets
import select
import signal
import subprocess
import sys
import tempfile
from pathlib import Path
sys.path.insert(0,"/bench")
from catdb_client import Client

def signature(lib,name,*args):
    f=getattr(lib,"ndb_tx_"+name); f.argtypes=args; f.restype=C.c_int
    return f

def smoke(root):
    lib=C.CDLL("/usr/local/lib/libnewdbms.so"); u=C.c_uint64; ptr=C.POINTER(u)
    create=signature(lib,"db_create",C.c_char_p,u,C.c_char_p,ptr)
    close=signature(lib,"db_close",u)
    configure=signature(lib,"set_checkpoint_range_writeback",u,C.c_uint32)
    begin=signature(lib,"begin",u,ptr,ptr)
    put=signature(lib,"put",u,C.c_char_p,u,C.c_char_p,u)
    commit=signature(lib,"commit",u,ptr)
    checkpoint=signature(lib,"checkpoint",u,ptr,ptr,ptr,ptr)
    path=root/"store";encoded=os.fsencode(path); db=u()
    assert create(encoded,len(encoded),bytes([75])*16,C.byref(db))==0
    assert configure(db,1)==0 and configure(db,2)==1
    for batch in range(9):
        tx=u();version=u()
        assert begin(db,C.byref(tx),C.byref(version))==0
        for slot in range(256):
            key=bytes((batch,slot)); val=bytes((slot,))*4096
            assert put(tx,key,len(key),val,len(val))==0
        assert commit(tx,C.byref(version))==0
    out=[u() for _ in range(4)]
    assert checkpoint(db,*(C.byref(i) for i in out))==0
    assert [v.value for v in out[:2]]==[9,2304]
    assert configure(db,0)==0 and close(db)==0 and configure(db,1)==1
    with tempfile.TemporaryDirectory(dir="/dev/shm") as tmp:
        bad=Path(tmp)/"store"; b=os.fsencode(bad)
        assert create(b,len(b),bytes([75])*16,C.byref(db))==0
        assert configure(db,1)==27 and configure(db,0)==0 and close(db)==0
        token=root/"token";secret=secrets.token_hex(24);token.write_text(secret)
        command=["catdb-server","--bind","127.0.0.1:0","--token-file",str(token),"--db"]
        for setting,parent in (("invalid",root),("1",Path(tmp))):
            new=parent/"rejected.db"
            r=subprocess.run(command+[str(new),"--init"],env={**os.environ,"CATDB_EXT4_CHECKPOINT_WRITEBACK":setting},
                             capture_output=True,text=True,timeout=15)
            assert r.returncode!=0 and not new.exists() and ("ext4" in r.stderr.lower() or "must be 0 or 1" in r.stderr),r.stderr
        for setting in ("1","0"):
            server=subprocess.Popen(command+[str(path)],env={**os.environ,"CATDB_EXT4_CHECKPOINT_WRITEBACK":setting},
                                    stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                assert select.select([server.stdout],[],[],20)[0]
                line=server.stdout.readline()
                assert "ready" in line,line
                port=int(line.split()[2].rsplit(":",1)[1])
                with Client("127.0.0.1",port,secret) as c:
                    info=c.request(op="checkpoint");assert info["ok"] and info["records"]==2304,info
                    for batch in range(9):
                        for slot in (0,127,255):
                            r=c.request(op="get_latest",key=bytes((batch,slot)).hex())
                            assert r["ok"] and r["value"]==(bytes((slot,))*4096).hex()
            finally:
                server.send_signal(signal.SIGINT)
                try:server.wait(timeout=10)
                except subprocess.TimeoutExpired:server.kill();server.wait();raise
                assert server.returncode==0
    print("Release C ABI and service OK: ext4, tmpfs, 9 MiB, checkpoint, reopen, invalid flag and closed handle.")

if __name__=="__main__":
    with tempfile.TemporaryDirectory(dir="/data") as folder:smoke(Path(folder))
