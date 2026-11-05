"""Track dispatch attempts, without treating an address as a transaction ID."""
import pathlib,sys,json,gzip,re,importlib.util,collections
spec=importlib.util.spec_from_file_location('sync_analysis',pathlib.Path(__file__).resolve().parent.parent/'sync-os/analyze.py');base=importlib.util.module_from_spec(spec);spec.loader.exec_module(base)
def requests(events):
 journal={};last={};generations=collections.Counter();rows=[];issues=[];before=collections.Counter();pending=collections.defaultdict(list)
 for e in events:
  b=e['body'];t=e['ns'];pid=e['pid']
  if b.startswith('jbd2_start_commit:'):
   dev,tx=re.search(r'dev (\d+,\d+) transaction (\d+)',b).groups();journal[pid]=dict(device=dev,transaction=int(tx))
  elif b.startswith('jbd2_end_commit:'):journal.pop(pid,None)
  if not b.startswith('rq_'):continue
  kind=b.split(':')[0];m=re.search(r'rq=(R\d+)',b)
  if not m:raise ValueError('missing opaque request identity')
  slot=m[1];a=last.get(slot)
  if kind=='rq_start':
   if a and a['state'] not in ('data_done','freed','requeued'):issues.append('start before observed completion/requeue')
   generations[slot]+=1
   a=dict(slot=slot,attempt=generations[slot],start_ns=t,issuer=e['comm'],journal_context=journal.get(pid),state='started',updates=[],ends=0,requeues=0)
   last[slot]=a;rows.append(a)
  elif kind=='rq_update':
   pending[(slot,pid)].append(dict(ns=t,bytes=int(re.search(r'bytes=(\d+)',b)[1]),error=int(re.search(r'error=(\d+)',b)[1]) if re.search(r'error=(\d+)',b) else None))
  elif kind=='rq_update_return':
   if not pending[(slot,pid)]:issues.append('return without entry');continue
   update=pending[(slot,pid)].pop();more=int(re.search(r'more=(\d+)',b)[1]);update.update(return_ns=t,more=more)
   if more not in (0,1) or t<update['ns']:raise ValueError('invalid completion return')
   if a is None:before[kind]+=1;continue
   if update['ns']<a['start_ns']:issues.append('update crosses dispatch boundary');continue
   a['updates'].append(update)
   if more==0:
    if 'data_end_ns' not in a:a['data_end_ns']=t;a['dispatch_to_data_done_ns']=t-a['start_ns']
    a['state']='data_done'
  elif a is None:before[kind]+=1
  elif kind=='rq_end':a['ends']+=1
  elif kind=='rq_requeue':a['requeues']+=1;a['state']='requeued'
  elif kind=='rq_free':a['state']='freed';a['free_ns']=t
 if any(pending.values()):issues.append('entry without return')
 return dict(attempts=rows,issues=issues,before_start=dict(before),reused_slots=sum(n>1 for n in generations.values()))
def main(folder):
 root=pathlib.Path(folder);out=[]
 for r in json.loads((root/'results.json').read_text()):
  if 'kernel_trace' not in r:continue
  path=root/r['kernel_trace'];meta=json.loads((path/'metadata.json').read_text())
  assert meta['cleaned'] and meta['request_probe_count']==6
  for stats in meta['stats'].values():
   for field in ['overrun','commit overrun','dropped events']:assert int(re.search(r'^'+field+r': (\d+)',stats,re.M)[1])==0,'trace loss'
  assert len(meta['probe_profile'])==6
  assert all(int(line.split()[-1])==0 for line in meta['probe_profile']),'missed probe instances'
  events=base.parse(path/'trace.txt.gz');a=requests(events);assert not a['issues'],a['issues']
  origin=r['schedule']['origin_ns'];end=r['schedule']['drained_ns']
  sync=base.analyze(path/'trace.txt.gz',origin,end);assert not sync['issues'] and len(sync['syncs'])==r['sync_attempts']
  a['attempts']=[x for x in a['attempts'] if origin<=x['start_ns']<=end]
  a['case']=r['kernel_trace'];a['syncs']=len(sync['syncs']);out.append(a)
 with gzip.open(root/'request-analysis.json.gz','wt') as f:json.dump(out,f)
 summaries=[]
 for a in out:
  rs=a['attempts'];complete=[x for x in rs if 'data_end_ns' in x]
  summaries.append(dict(case=a['case'],syncs=a['syncs'],dispatches=len(rs),data_done=len(complete),unresolved_data_end=len(rs)-len(complete),direct_journal_context=sum(x['journal_context'] is not None for x in rs),unassigned_journal_context=sum(x['journal_context'] is None for x in rs),reused_slots=a['reused_slots'],partial_updates=sum(u['more']==1 for x in rs for u in x['updates']),requeues=sum(x['requeues'] for x in rs),without_end_hook=sum(x['ends']==0 for x in complete),worst=max(complete,key=lambda x:x['dispatch_to_data_done_ns'])))
 (root/'summary.json').write_text(json.dumps(summaries,indent=2));print(json.dumps(summaries,indent=2))
if __name__=='__main__':main(sys.argv[1])
