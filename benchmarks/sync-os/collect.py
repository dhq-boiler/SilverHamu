"""Bounded trace instance; never enables the global tracing instance."""
import gzip,json,pathlib,subprocess,time,uuid
root=pathlib.Path('/trace');out=pathlib.Path('/control')
subprocess.run(['mount','-t','tracefs','tracefs',str(root)],check=True)
instance=root/'instances'/('catdb-'+uuid.uuid4().hex)
instance.mkdir()
def put(name,value): (instance/name).write_text(str(value))
events=['syscalls/sys_enter_fsync','syscalls/sys_exit_fsync','sched/sched_switch','sched/sched_wakeup','block/block_rq_issue','block/block_rq_complete']
meta={'clock':'mono','instance':instance.name,'events':events,'started_monotonic_ns':time.monotonic_ns(),'buffer_kb_per_cpu':16384}
try:
 put('tracing_on',0);put('trace_clock','mono');put('buffer_size_kb',16384)
 put('events/sched/sched_switch/filter','prev_comm == "silverhamu-server" || next_comm == "silverhamu-server" || prev_comm == "catdb-client" || next_comm == "catdb-client"')
 put('events/sched/sched_wakeup/filter','comm == "silverhamu-server" || comm == "catdb-client"')
 put('events/sched/sched_switch/trigger','stacktrace if (prev_comm == "silverhamu-server" || prev_comm == "catdb-client") && prev_state & 2')
 for event in events:put('events/'+event+'/enable',1)
 put('tracing_on',1)
 (out/'ready').write_text(instance.name)
 deadline=time.monotonic()+180
 while not (out/'stop').exists():
  if time.monotonic()>deadline:raise TimeoutError('bounded capture expired')
  time.sleep(.1)
finally:
 put('tracing_on',0)
 meta['stopped_monotonic_ns']=time.monotonic_ns()
 try:
  meta['stats']={p.parent.name:p.read_text() for p in (instance/'per_cpu').glob('cpu*/stats')}
  with gzip.open(out/'trace.txt.gz','wb') as dest:
   with (instance/'trace').open('rb') as source:
    while chunk:=source.read(1024*1024):dest.write(chunk)
 finally:
  try:
   for event in events:put('events/'+event+'/enable',0)
   put('events/sched/sched_switch/trigger','!stacktrace')
  finally:instance.rmdir()
 meta['cleaned']=True
 (out/'metadata.json').write_text(json.dumps(meta,indent=2))
