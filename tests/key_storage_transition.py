"""EQ/ST: both layouts read each other's WAL and checkpoint without conversion."""
import ctypes as c
import os
import pathlib
import subprocess
import sys
import tempfile


def fixture_key(i):
    return i.to_bytes(8,'big') + (bytes(range(24)) if i % 2 else b'')


def phase(library, path, step):
    n = c.CDLL(str(library)); u = c.c_uint64; ptr = c.POINTER(u)
    for name, args in {
        'db_create': [c.c_char_p,u,c.c_char_p,ptr],
        'db_open': [c.c_char_p,u,ptr], 'db_close': [u],
        'begin': [u,ptr,ptr], 'get': [u,c.c_char_p,u,c.c_void_p,u,ptr,ptr],
        'put': [u,c.c_char_p,u,c.c_char_p,u], 'delete': [u,c.c_char_p,u],
        'commit': [u,ptr], 'rollback': [u], 'checkpoint': [u,ptr,ptr,ptr,ptr],
    }.items():
        getattr(n,'ndb_tx_'+name).argtypes=args
    def call(name,*args):
        status=getattr(n,'ndb_tx_'+name)(*args)
        assert status==0,(name,status,step)
    encoded=os.fsencode(path); db=u(); version=u()
    if step==0:
        create = getattr(n, 'ndb_tx_db_create_append', n.ndb_tx_db_create)
        create.argtypes = n.ndb_tx_db_create.argtypes
        assert create(encoded,len(encoded),bytes(range(16)),c.byref(db)) == 0
    else: call('db_open',encoded,len(encoded),c.byref(db))
    def begin():
        tx=u(); call('begin',db,c.byref(tx),c.byref(version)); return tx
    def expected(g):
        return {b'':b'',b'large':bytes(range(256))*1024,
                **{fixture_key(i):bytes([g])*256 for i in range(400) if i%4!=g%4}}
    if step:
        tx=begin(); want=expected(step-1)
        for key in [b'',b'large',*[fixture_key(i) for i in range(400)]]:
            buf=c.create_string_buffer(262144); needed=u(); found=u()
            call('get',tx,key,len(key),buf,len(buf),c.byref(needed),c.byref(found))
            assert bool(found.value)==(key in want)
            if key in want: assert buf.raw[:needed.value]==want[key]
        call('rollback',tx)
    if step<4:
        tx=begin(); want=expected(step)
        for i in range(400):
            key=fixture_key(i)
            if key not in want: call('delete',tx,key,len(key))
        for key,value in want.items():call('put',tx,key,len(key),value,len(value))
        call('commit',tx,c.byref(version))
        if step%2:
            outputs=[u() for _ in range(4)]
            call('checkpoint',db,*[c.byref(v) for v in outputs])
    call('db_close',db)
    if step == 0:
        with path.open('rb') as wal:
            wal.seek(8)
            assert int.from_bytes(wal.read(4), 'little') == 2


def main():
    if len(sys.argv)==5 and sys.argv[1]=='phase':
        phase(pathlib.Path(sys.argv[2]),pathlib.Path(sys.argv[3]),int(sys.argv[4])); return
    assert len(sys.argv)==3,'usage: storage_transition.py DEQUE_LIBRARY DEFAULT_LIBRARY'
    libs=[str(pathlib.Path(x).resolve()) for x in sys.argv[1:]]
    root=pathlib.Path(__file__).resolve().parents[1]/'artifacts';root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='layout-compat-',dir=root) as tmp:
        for first in (0,1):
            path=pathlib.Path(tmp)/f'order-{first}.wal'
            for step in range(5):
                subprocess.run([sys.executable,__file__,'phase',libs[(first+step)%2],str(path),str(step)],check=True)
    print('PASS: 2 directions, 8 full-content checks, WAL and checkpoint interchange')

if __name__=='__main__':main()
