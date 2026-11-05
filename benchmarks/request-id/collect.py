"""Bounded trace instance; never enables the global tracing instance."""
import gzip,json,pathlib,subprocess,time,uuid,re,os
root=pathlib.Path('/trace');out=pathlib.Path('/control')
subprocess.run(['mount','-t','tracefs','tracefs',str(root)],check=True)
instance=root/'instances'/('catdb-'+uuid.uuid4().hex)
instance.mkdir()
group='catdb_rq_'+uuid.uuid4().hex
registered=[]
probes={'rq_start':('blk_mq_start_request',''), 'rq_end':('blk_mq_end_request',' error=$arg2:u32'), 'rq_update':('blk_update_request',' error=$arg2:u32 bytes=$arg3:u32'), 'rq_free':('blk_mq_free_request',''), 'rq_requeue':('blk_mq_requeue_request',''), 'rq_update_return':('blk_update_request%return',' more=$retval:u8')}
def define(command):
 fd=os.open(root/'kprobe_events',os.O_WRONLY|os.O_APPEND)
 try:os.write(fd,(command+'\n').encode())
 finally:os.close(fd)

def put(name,value): (instance/name).write_text(str(value))
events=['syscalls/sys_enter_fsync','syscalls/sys_exit_fsync','sched/sched_switch','sched/sched_wakeup','block/block_rq_issue','block/block_rq_complete','block/block_rq_requeue','block/block_rq_merge','jbd2/jbd2_start_commit','jbd2/jbd2_commit_locking','jbd2/jbd2_commit_flushing','jbd2/jbd2_commit_logging','jbd2/jbd2_end_commit','ext4/ext4_sync_file_enter','ext4/ext4_sync_file_exit']
meta={'clock':'mono','instance':instance.name,'events':events,'started_monotonic_ns':time.monotonic_ns(),'buffer_kb_per_cpu':16384}
try:
 existing=(root/'kprobe_events').read_text()
 if any('/'+name+' ' in existing for name in probes):raise RuntimeError('probe event name already in use')
 for name,(symbol,extra) in probes.items():
  define(f'p:{group}/{name} {symbol} rq=$arg1:x64'+extra)
  registered.append(name);events.append(group+'/'+name)
 meta['request_probe_count']=len(registered)
 put('tracing_on',0);put('trace_clock','mono');put('buffer_size_kb',16384)
 put('events/sched/sched_switch/filter','prev_comm == "catdb-server" || next_comm == "catdb-server" || prev_comm == "catdb-client" || next_comm == "catdb-client" || prev_comm ~ "jbd2/*" || next_comm ~ "jbd2/*"')
 put('events/sched/sched_wakeup/filter','comm == "catdb-server" || comm == "catdb-client" || comm ~ "jbd2/*"')
 for event in events:put('events/'+event+'/enable',1)
 put('tracing_on',1)
 (out/'ready').write_text(instance.name)
 deadline=time.monotonic()+180
 while not (out/'stop').exists():
  if time.monotonic()>deadline:raise TimeoutError('bounded capture expired')
  time.sleep(.1)
except BaseException as error:
 meta['error']=repr(error)
 raise
finally:
 put('tracing_on',0)
 meta['stopped_monotonic_ns']=time.monotonic_ns()
 meta['probe_profile']=[line for line in (root/'kprobe_profile').read_text().splitlines() if line.split() and line.split()[0] in registered]
 try:
  meta['stats']={p.parent.name:p.read_text() for p in (instance/'per_cpu').glob('cpu*/stats')}
  ids={}
  def opaque(match):
   pointer=match[1]
   if pointer not in ids:ids[pointer]='R'+str(len(ids)+1).zfill(6)
   return 'rq='+ids[pointer]
  with gzip.open(out/'trace.txt.gz','wt') as dest:
   with (instance/'trace').open() as source:
    for line in source:
     line=re.sub(r'rq=(0x[0-9a-fA-F]+)',opaque,line)
     dest.write(re.sub(r'0x[fF][0-9a-fA-F]{15}\b','<address-redacted>',line))
  meta['opaque_request_slots']=len(ids)
 finally:
  try:
   for event in events:
    path=instance/'events'/event/'enable'
    if path.exists():path.write_text('0')
  finally:
   try:instance.rmdir()
   finally:
    for name in reversed(registered):define(f'-:{group}/{name}')
 meta['cleaned']=True
 (out/'metadata.json').write_text(json.dumps(meta,indent=2))
