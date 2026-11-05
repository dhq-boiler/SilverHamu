"""Partition each fsync into scheduled, blocked and runnable intervals."""
import gzip,json,re,pathlib,sys
LINE=re.compile(r'^\s*(.+)-(\d+)\s+\[(\d+)\]\s+\S+\s+(\d+)\.(\d+): (.*)$')
def parse(path):
 events=[]
 with gzip.open(path,'rt') as f:
  for line in f:
   m=LINE.match(line)
   if m:
    comm,pid,cpu,sec,frac,body=m.groups()
    events.append(dict(comm=comm.strip(),pid=int(pid),cpu=int(cpu),ns=int(sec)*10**9+int(frac.ljust(9,'0')),body=body,stack=[]))
   elif line.startswith(' => ') and events:events[-1]['stack'].append(line[4:].strip())
 return events
def analyze(path,start,end):
 events=parse(path);active={};rows=[];issues=[];block_count=0
 def transition(tid,t,state=None):
  a=active.get(tid)
  if not a:return
  if t<a['last']:raise ValueError('time reversed')
  a[a['state']]+=t-a['last'];a['last']=t
  if state:a['state']=state
 for e in events:
  b=e['body'];t=e['ns'];pid=e['pid']
  if 'block_rq_' in b and start<=t<=end:block_count+=1
  if b.startswith('sys_fsync(fd:') and e['comm']=='catdb-client' and start<=t<=end:
   if pid in active:raise ValueError('nested fsync')
   active[pid]=dict(tid=pid,start_ns=t,last=t,state='scheduled_ns',scheduled_ns=0,runnable_ns=0,blocked_ns=0,io_stack_blocked_ns=0,other_blocked_ns=0,stacks=[])
  elif b.startswith('sys_fsync ->') and pid in active:
   transition(pid,t);a=active.pop(pid);a['end_ns']=t;a['duration_ns']=t-a['start_ns'];a['return']=b.split('-> ')[1]
   assert a['duration_ns']==a['scheduled_ns']+a['runnable_ns']+a['blocked_ns']
   a.pop('last');a.pop('state');rows.append(a)
  elif b.startswith('sched_switch:'):
   m=re.search(r'prev_pid=(\d+).*prev_state=(\S+).*next_pid=(\d+)',b)
   prev,state,nxt=m.groups();prev=int(prev);nxt=int(nxt)
   if prev in active:
    transition(prev,t,'runnable_ns' if state.startswith('R') else 'blocked_ns')
    if not state.startswith('R'):active[prev]['sleep_start']=t;active[prev]['sleep_stack']=[]
   if nxt in active:
    if active[nxt]['state']=='blocked_ns':issues.append('scheduled without wakeup')
    transition(nxt,t,'scheduled_ns')
  elif b.startswith('sched_wakeup:'):
   tid=int(re.search(r' pid=(\d+)',b)[1]);a=active.get(tid)
   if a and a['state']=='blocked_ns':
    stack=a.pop('sleep_stack',[]);duration=t-a.pop('sleep_start')
    kind='io_stack_blocked_ns' if any(x.startswith(('io_schedule','jbd2_log_wait_commit','folio_wait_writeback','wait_on_page_writeback')) for x in stack) else 'other_blocked_ns'
    a[kind]+=duration;a['stacks'].append(dict(start_ns=t-duration,end_ns=t,kind=kind,stack=stack))
    transition(tid,t,'runnable_ns')
  elif b=='<stack trace>' and pid in active and active[pid]['state']=='blocked_ns':active[pid]['sleep_stack']=e['stack']
 if active:issues.append('incomplete fsync')
 return dict(syncs=rows,issues=issues,block_events=block_count)
def main(folder):
 root=pathlib.Path(folder);results=json.loads((root/'results.json').read_text());out=[]
 for r in results:
  if 'kernel_trace' not in r:continue
  p=root/r['kernel_trace'];meta=json.loads((p/'metadata.json').read_text())
  for value in meta['stats'].values():
   for field in ['overrun','commit overrun','dropped events']:
    assert int(re.search(r'^'+field+r': (\d+)',value,re.M)[1])==0,'trace loss'
  assert meta['cleaned']
  a=analyze(p/'trace.txt.gz',r['schedule']['origin_ns'],r['schedule']['drained_ns'])
  assert len(a['syncs'])==r['sync_attempts'],(len(a['syncs']),r['sync_attempts'])
  assert not a['issues'],a['issues']
  a.update(case=r['kernel_trace'],clients=r['clients'],rate=r['rate'],rep=r['rep'])
  out.append(a)
 with gzip.open(root/'kernel-analysis.json.gz','wt') as f:json.dump(out,f)
 print(json.dumps([dict(case=a['case'],syncs=len(a['syncs']),worst=max(a['syncs'],key=lambda x:x['duration_ns'])) for a in out],indent=2))
if __name__=='__main__':main(sys.argv[1])
