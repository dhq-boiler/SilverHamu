"""Attribute journal scheduling without inventing block-request ownership."""
import pathlib,sys,json,gzip,re,importlib.util
base=pathlib.Path(__file__).resolve().parent.parent/'sync-os/analyze.py'
spec=importlib.util.spec_from_file_location('sync_analysis',base);previous=importlib.util.module_from_spec(spec);spec.loader.exec_module(previous)
def journals(events,start,end):
 active={};rows=[];issues=[]
 def advance(pid,t,state=None):
  a=active.get(pid)
  if a is None:return
  if t<a['last']:raise ValueError('time reversal')
  a[a['state']]+=t-a['last'];a['last']=t
  if state:a['state']=state
 for e in events:
  b=e['body'];t=e['ns'];pid=e['pid']
  if b.startswith('jbd2_start_commit:') and start<=t<=end:
   if pid in active:raise ValueError('overlapping journal transaction')
   dev,tx=re.search(r'dev (\d+,\d+) transaction (\d+)',b).groups()
   active[pid]=dict(tid=pid,device=dev,transaction=int(tx),start_ns=t,last=t,state='scheduled_ns',scheduled_ns=0,runnable_ns=0,blocked_ns=0,phases=[['start',t]])
  elif b.startswith('jbd2_') and pid in active:
   a=active[pid];dev,tx=re.search(r'dev (\d+,\d+) transaction (\d+)',b).groups()
   if (dev,int(tx))!=(a['device'],a['transaction']):raise ValueError('journal identity mismatch')
   phase=b.split(':')[0].replace('jbd2_commit_','').replace('jbd2_end_commit','end')
   expected=['start','locking','flushing','logging','end']
   if len(a['phases'])>=5 or phase!=expected[len(a['phases'])]:raise ValueError('journal phase order')
   a['phases'].append([phase,t])
   if phase=='end':
    advance(pid,t);a=active.pop(pid);a['end_ns']=t;a['duration_ns']=t-a['start_ns'];a.pop('last');a.pop('state')
    assert a['duration_ns']==a['scheduled_ns']+a['blocked_ns']+a['runnable_ns'];rows.append(a)
  elif b.startswith('sched_switch:'):
   prev,state,nxt=re.search(r'prev_pid=(\d+).*prev_state=(\S+).*next_pid=(\d+)',b).groups();prev=int(prev);nxt=int(nxt)
   advance(prev,t,'runnable_ns' if state.startswith('R') else 'blocked_ns')
   if nxt in active:
    if active[nxt]['state']=='blocked_ns':issues.append('missing wakeup')
    advance(nxt,t,'scheduled_ns')
  elif b.startswith('sched_wakeup:'):
   target=int(re.search(r' pid=(\d+)',b)[1])
   if target in active and active[target]['state']=='blocked_ns':advance(target,t,'runnable_ns')
 if active:issues.append('incomplete journal transaction')
 return dict(transactions=rows,issues=issues)
def main(folder):
 root=pathlib.Path(folder);out=[]
 for r in json.loads((root/'results.json').read_text()):
  if 'kernel_trace' not in r:continue
  trace=root/r['kernel_trace'];meta=json.loads((trace/'metadata.json').read_text())
  assert meta['cleaned'] and 'jbd2/jbd2_start_commit' in meta['events']
  for stats in meta['stats'].values():
   for field in ['overrun','commit overrun','dropped events']:assert int(re.search(r'^'+field+r': (\d+)',stats,re.M)[1])==0,'trace loss'
  events=previous.parse(trace/'trace.txt.gz');start=r['schedule']['origin_ns'];end=r['schedule']['drained_ns']
  assert not any(e['body']=='<stack trace>' for e in events)
  sync=previous.analyze(trace/'trace.txt.gz',start,end)
  assert not sync['issues'] and len(sync['syncs'])==r['sync_attempts']
  a=journals(events,start,end);assert not a['issues'],a['issues']
  a.update(case=r['kernel_trace'],syncs=len(sync['syncs']),block_events=[e for e in events if start<=e['ns']<=end and e['body'].startswith('block_rq_')],ownership='unresolved: no request ID or journal transaction ID in block events')
  out.append(a)
 with gzip.open(root/'journal-analysis.json.gz','wt') as f:json.dump(out,f)
 summary=[dict(case=a['case'],syncs=a['syncs'],journals=len(a['transactions']),worst=max(a['transactions'],key=lambda x:x['duration_ns']),max_runnable_ns=max(x['runnable_ns'] for x in a['transactions']),block_events=len(a['block_events'])) for a in out]
 (root/'summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))
if __name__=='__main__':main(sys.argv[1])
