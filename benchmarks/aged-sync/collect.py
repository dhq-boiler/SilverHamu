"""Private bounded trace, filtered to the already connected target DB threads."""
import gzip,json,os,pathlib,re,subprocess,time,uuid
from identity import target_ids,exportable
root=pathlib.Path('/trace');out=pathlib.Path('/control');parent=int(os.environ['TARGET_SUPERVISOR'])
subprocess.run(['mount','-t','tracefs','tracefs',str(root)],check=True)
children=pathlib.Path(f'/proc/{parent}/task/{parent}/children').read_text().split()
servers=[int(p) for p in children if pathlib.Path(f'/proc/{p}/comm').read_text().strip()=='catdb-server']
assert len(servers)==1,servers
tag=os.environ['TARGET_THREAD_NAME'];assert re.fullmatch(r'[a-z0-9-]{1,15}',tag)
pid=servers[0];tids=sorted(int(p.name) for p in pathlib.Path(f'/proc/{pid}/task').iterdir())
assert 5<=len(tids)<=128,tids
tagged=[]
for proc in pathlib.Path('/proc').glob('[0-9]*'):
    try:
        for task in (proc/'task').iterdir():
            if (task/'comm').read_text().strip()==tag:tagged.append((int(proc.name),int(task.name)))
    except (OSError,ProcessLookupError):pass
assert len(tagged)>=4 and all(owner==pid for owner,tid in tagged),tagged
fds={int(p.name):os.readlink(p) for p in pathlib.Path(f'/proc/{pid}/fd').iterdir() if p.is_symlink()}
wal=[fd for fd,target in fds.items() if target=='/data/catdb.wal'];assert len(wal)==1,wal
instance=root/'instances'/('catdb-aged-sync-'+uuid.uuid4().hex);instance.mkdir()
events=['syscalls/sys_enter_fsync','syscalls/sys_exit_fsync','syscalls/sys_enter_fdatasync','syscalls/sys_exit_fdatasync','sched/sched_switch','sched/sched_wakeup']
meta={'pid':pid,'namespace_tids':tids,'thread_tag':tag,'wal_fd':wal[0],'instance':instance.name,'clock':'mono','events':events,'buffer_kb_per_cpu':2048,'target_start_stat':pathlib.Path(f'/proc/{pid}/stat').read_text(),'cleaned':False}
def put(name,value):(instance/name).write_text(str(value))
try:
    put('tracing_on',0);put('trace_clock','mono');put('buffer_size_kb',2048)
    cpus=list((instance/'per_cpu').glob('cpu*'));assert len(cpus)*2*1024*1024<=64*1024*1024,'trace memory ceiling'
    meta['trace_buffer_bytes']=len(cpus)*2*1024*1024
    put('events/sched/sched_switch/filter',f'prev_comm == "{tag}" || next_comm == "{tag}"')
    put('events/sched/sched_wakeup/filter',f'comm == "{tag}"')
    for e in events:put('events/'+e+'/enable',1)
    meta['start_ns']=time.monotonic_ns();put('tracing_on',1);(out/'ready').write_text(instance.name)
    deadline=time.monotonic()+90
    while not (out/'stop').exists():
        if time.monotonic()>deadline:raise TimeoutError('bounded trace expired')
        time.sleep(.05)
finally:
    put('tracing_on',0);meta['end_ns']=time.monotonic_ns()
    try:
        meta['stats']={p.parent.name:p.read_text() for p in (instance/'per_cpu').glob('cpu*/stats')}
        size=0;kept=0;kernel_tids=set()
        with (instance/'trace').open() as src:
            for line in src:
                size+=len(line.encode())
                if size>64*1024*1024:raise RuntimeError('trace export ceiling')
                kernel_tids.update(target_ids(line,tag))
        assert kernel_tids,'no target kernel thread IDs'
        with gzip.open(out/'trace.txt.gz','wt') as dest:
            with (instance/'trace').open() as src:
                for line in src:
                    if exportable(line,kernel_tids):dest.write(line);kept+=len(line.encode())
        meta['uncompressed_bytes']=size;meta['exported_bytes']=kept;meta['tids']=sorted(kernel_tids)
    finally:
        for e in events:put('events/'+e+'/enable',0)
        instance.rmdir();meta['cleaned']=True
        (out/'metadata.json').write_text(json.dumps(meta,indent=2))
