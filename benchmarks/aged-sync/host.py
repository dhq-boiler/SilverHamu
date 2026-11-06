"""Synchronize capture after all worker connections exist, before timed arrival."""
import concurrent.futures,json,pathlib,subprocess,time,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
def command(*a,timeout=60):
    p=subprocess.run(list(map(str,a)),capture_output=True,text=True,timeout=timeout)
    if p.returncode:raise RuntimeError(p.stderr[-2000:]+p.stdout[-1000:])
    return p.stdout.strip()
def measure(execute,work,ops,rate,pin,server,image,folder,mode):
    folder.mkdir(exist_ok=True);collector=None
    pool=concurrent.futures.ThreadPoolExecutor(max_workers=1)
    future=pool.submit(execute,'measure','--work',work,'--ops',str(ops),'--rate',str(rate),'--pin-reader',str(pin))
    try:
        deadline=time.monotonic()+30
        while not (folder/'armed').exists():
            if future.done():return future.result()
            if time.monotonic()>deadline:raise TimeoutError('client failed to arm')
            time.sleep(.02)
        release_at=time.monotonic()+3
        if mode=='trace':
            parent=command('docker','inspect',server,'--format','{{.State.Pid}}')
            tag=command('docker','exec',server,'printenv','CATDB_BENCH_THREAD_NAME')
            collector='catdb-aged-sync-'+uuid.uuid4().hex[:12]
            command('docker','run','-d','--name',collector,'--network','none','--pid','host','--read-only','--cap-add','SYS_ADMIN','--memory','128m','--memory-swap','128m','--tmpfs','/trace:rw,nosuid,nodev,mode=700','-e','TARGET_SUPERVISOR='+parent,'-e','TARGET_THREAD_NAME='+tag,'-v',str(folder)+':/control','-v',str(ROOT/'benchmarks/aged-sync')+':/collector:ro',image,'python','/collector/collect.py')
            deadline=time.monotonic()+20
            while not (folder/'ready').exists():
                running=command('docker','inspect',collector,'--format','{{.State.Running}}')
                if running!='true':raise RuntimeError('collector failed: '+command('docker','logs',collector))
                if time.monotonic()>deadline:raise TimeoutError('collector readiness expired')
                time.sleep(.05)
        if time.monotonic()>release_at:raise TimeoutError('collector exceeded common setup interval')
        time.sleep(max(0,release_at-time.monotonic()))
        (folder/'setup.json').write_text(json.dumps({'common_setup_seconds':3,'mode':mode}))
        (folder/'go').write_text('go')
        return future.result(timeout=120)
    finally:
        if collector:
            (folder/'stop').write_text('stop')
            try:
                status=command('docker','wait',collector,timeout=30)
                if status!='0':raise RuntimeError('collector failed: '+command('docker','logs',collector))
            finally:command('docker','rm','-f',collector)
        pool.shutdown(wait=False,cancel_futures=True)
