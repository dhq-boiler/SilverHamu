"""Kill a disposable server with transactions in flight; check every acknowledged value and transaction atomicity."""
import argparse
import concurrent.futures
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import struct
import subprocess
import threading
import time
import uuid

def docker(*args, env=None, check=True):
    result=subprocess.run(['docker',*args], env=env, capture_output=True, text=True, timeout=90)
    if check and result.returncode:
        raise RuntimeError(result.stderr)
    return result.stdout.strip()

def checked(client, **request):
    result = client.request(**request)
    if not result.get('ok'):
        raise RuntimeError(str(result))
    return result

def main():
    p=argparse.ArgumentParser()
    p.add_argument('--repo',type=Path,default=Path(__file__).resolve().parents[2])
    p.add_argument('--image',required=True)
    p.add_argument('--out',type=Path,required=True)
    p.add_argument('--server-env',action='append',default=[])
    p.add_argument('--expect-format', type=int)
    args=p.parse_args()
    args.out.mkdir(parents=True,exist_ok=False)
    spec=importlib.util.spec_from_file_location('catdb_client',args.repo/'scripts/catdb-client.py')
    clientmod=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(clientmod)
    name='catdb-crash-'+uuid.uuid4().hex[:12]
    volume=name+'-data'
    secret=secrets.token_hex(32)
    switches=[]
    runtime={}
    for item in args.server_env:
        key,value=item.split('=',1)
        assert key.isidentifier() and key.startswith('CATDB_') and key not in runtime
        runtime[key]=value; switches.extend(['-e',key])
    env=dict(os.environ,**runtime)
    report={'image':docker('image','inspect',args.image,'--format','{{.Id}}'),'server_env':runtime,'rounds':[]}
    docker('volume','create',volume)
    def connect():
        return clientmod.Client('127.0.0.1',port,secret,timeout=10)
    def ready():
        nonlocal port
        port=int(docker('port',name,'7400/tcp').splitlines()[0].rsplit(':',1)[1])
        deadline=time.monotonic()+60
        while True:
            try:
                with connect() as c: checked(c,op='ping')
                return
            except (ConnectionError, OSError):
                if time.monotonic()>=deadline: raise RuntimeError(docker('logs',name))
                time.sleep(.1)
    def actual_format():
        script="import struct; f=open('/data/catdb.wal','rb'); print(struct.unpack_from('<I',f.read(12),8)[0])"
        version=int(docker('exec',name,'python','-c',script))
        if args.expect_format is not None: assert version == args.expect_format,version
        return version
    def scan():
        rows={}
        with connect() as c:
            checked(c,op='begin_read')
            after=None
            while True:
                request={'op':'scan','start':'','rows':100,'bytes':64000,'visits':65536}
                if after is not None: request['after']=after
                page=checked(c,**request)
                for k,v in page['rows']:
                    assert k not in rows
                    rows[k]=v
                after=page['next_after']
                if after is None: break
            checked(c,op='rollback')
        return rows
    port=0
    try:
        init="import os,pathlib; p=pathlib.Path('/data/token'); p.write_text(os.environ['BENCH_TOKEN']); p.chmod(0o400); os.chown(p,10001,10001); os.chown('/data',10001,10001)"
        docker('run','--rm','--network','none','-v',volume+':/data','-e','BENCH_TOKEN',args.image,'python','-c',init,env=dict(os.environ,BENCH_TOKEN=secret))
        docker('run','--detach','--name',name,'--cpus','2','--memory','4g','--user','10001:10001','-p','127.0.0.1::7400','-v',volume+':/data',*switches,args.image,'catdb-server','--db','/data/catdb.wal','--init','--token-file','/data/token','--bind','0.0.0.0:7400','--allow-remote',env=env)
        ready()
        report['initial_format']=actual_format()
        known={}
        for epoch, clients in enumerate((4,16),1):
            attempts={};acks=set();guard=threading.Lock()
            threshold=threading.Event();start=threading.Barrier(clients+1, timeout=30)
            stopping=threading.Event();stop_loop=threading.Event()
            def write(worker):
                with connect() as c:
                    start.wait()
                    counter=0
                    while not stop_loop.is_set():
                        slots=2 if counter%2 else 1
                        group={}
                        for slot in range(slots):
                            key=struct.pack('>BBIH',epoch,worker,counter,slot)
                            group[key.hex()] = (hashlib.sha256(key).digest()*8).hex()
                        tx=(worker,counter)
                        with guard: attempts[tx]=group
                        try:
                            if slots==1:
                                k,v=next(iter(group.items()));checked(c,op='put_commit',key=k,value=v)
                            else:
                                checked(c,op='begin')
                                for k,v in group.items(): checked(c,op='put',key=k,value=v)
                                checked(c,op='commit')
                        except (OSError,ConnectionError):
                            if not stopping.is_set(): raise
                            return
                        with guard:
                            acks.add(tx)
                            if len(acks)>=128:threshold.set()
                        counter+=1
            with connect() as staged:
                checked(staged,op='begin');checked(staged,op='put',key='ffff',value='01')
                with concurrent.futures.ThreadPoolExecutor(max_workers=clients) as pool:
                    workers=[pool.submit(write,i) for i in range(clients)]
                    try:
                        start.wait();assert threshold.wait(30),'no commit progress'
                        stopping.set()
                        docker('kill','--signal','KILL',name)
                    finally: stop_loop.set()
                    for w in workers:w.result(timeout=30)
            assert docker('inspect','--format','{{.State.ExitCode}}',name)=='137'
            docker('start',name);ready()
            rows=scan()
            assert 'ffff' not in rows
            assert all(rows.get(k)==v for k,v in known.items()),'prior recovered state lost'
            possible=known.copy()
            recovered_groups=0
            for tx,group in attempts.items():
                present=sum(k in rows for k in group)
                assert present in (0,len(group)),('partial transaction',tx)
                assert tx not in acks or present==len(group),('acknowledged transaction absent',tx)
                if present:
                    assert all(rows[k]==v for k,v in group.items())
                    recovered_groups+=1
                possible.update(group)
            assert set(rows)<=set(possible) and recovered_groups>=len(acks)
            report['rounds'].append({'connections':clients,'submitted_or_staged':len(attempts),'acknowledged':len(acks),'recovered_transactions':recovered_groups,'recovered_keys':len(rows),'uncommitted_key_absent':True})
            known=rows
            with connect() as c: checked(c,op='checkpoint')
            assert scan()==known
            report['rounds'][-1]['checkpoint_format']=actual_format()
        report['status']='passed'
    finally:
        docker('rm','--force',name,check=False);docker('volume','rm',volume,check=False)
        (args.out/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__=='__main__':main()
