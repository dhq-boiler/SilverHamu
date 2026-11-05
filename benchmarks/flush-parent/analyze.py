"""Observe sequence callbacks nested inside a particular flush completion."""
import collections,gzip,importlib.util,json,pathlib,re,sys
spec=importlib.util.spec_from_file_location('requests',pathlib.Path(__file__).resolve().parent.parent/'request-id/analyze.py');rq=importlib.util.module_from_spec(spec);spec.loader.exec_module(rq)
spec=importlib.util.spec_from_file_location('journals',pathlib.Path(__file__).resolve().parent.parent/'journal-wait/analyze.py');ja=importlib.util.module_from_spec(spec);spec.loader.exec_module(ja)

def parents(events):
 journal={}; inserted={}; serial=collections.Counter(); dispatch=collections.Counter(); last_start={}; stacks=collections.defaultdict(list); rows=[]; issues=[]
 for e in events:
  b=e['body'];t=e['ns'];pid=e['pid'];ctx=(e['cpu'],pid)
  if b.startswith('jbd2_start_commit:'):
   d,x=re.search(r'dev (\d+,\d+) transaction (\d+)',b).groups();journal[pid]=dict(device=d,transaction=int(x))
  elif b.startswith('jbd2_end_commit:'):journal.pop(pid,None)
  if not b.startswith(('rq_','fp_')):continue
  kind=b.split(':')[0];slot=re.search(r'rq=(R\d+)',b)[1]
  if kind=='rq_start':dispatch[slot]+=1;last_start[slot]=(dispatch[slot],t)
  elif kind=='fp_insert':
   serial[slot]+=1;inserted[slot]=dict(slot=slot,insertion=serial[slot],ns=t,journal=journal.get(pid))
  elif kind=='rq_free':inserted.pop(slot,None)
  elif kind=='fp_end':
   a=dict(flush=slot,dispatch=last_start.get(slot),entry_ns=t,members=[],error=int(re.search(r'error=(\d+)',b)[1]));stacks[ctx].append(a);rows.append(a)
  elif kind=='fp_seq':
   seq=int(re.search(r'seq=(\d+)',b)[1]);error=int(re.search(r'error=(\d+)',b)[1])
   if stacks[ctx] and seq in (1,4):
    a=stacks[ctx][-1];parent=inserted.get(slot)
    a['members'].append(dict(parent=parent,slot=slot,phase=seq,ns=t,error=error,queue=re.search(r'fq=(R\d+)',b)[1]))
  elif kind=='fp_return':
   if not stacks[ctx]:issues.append('return without flush entry');continue
   a=stacks[ctx].pop()
   if a['flush']!=slot or t<a['entry_ns']:issues.append('mismatched flush return')
   a['return_ns']=t
   if len({(m['slot'],m['phase']) for m in a['members']})!=len(a['members']):issues.append('duplicate member')
 if any(stacks.values()):issues.append('incomplete flush callback')
 return dict(callbacks=rows,issues=issues)

def quality(meta):
 errors=[]
 if not meta.get('cleaned') or meta.get('request_probe_count')!=10:errors.append('incomplete collector')
 profiles=meta.get('probe_profile',[])
 if len(profiles)!=10 or any(int(x.split()[-1])!=0 for x in profiles):errors.append('missing or missed probes')
 for s in meta.get('stats',{}).values():
  for f in ['overrun','commit overrun','dropped events']:
   m=re.search('^'+f+r': (\d+)',s,re.M)
   if not m or int(m[1])!=0:errors.append('ring loss or missing counters')
 if not meta.get('stats'):errors.append('missing ring counters')
 return errors

def main(folder):
 root=pathlib.Path(folder);summaries=[];details=[]
 for r in json.loads((root/'results.json').read_text()):
  if 'kernel_trace' not in r:continue
  p=root/r['kernel_trace'];meta=json.loads((p/'metadata.json').read_text())
  errors=quality(meta)
  if errors:
   summaries.append(dict(case=r['kernel_trace'],valid=False,quality_errors=errors,gate=False));continue
  events=rq.base.parse(p/'trace.txt.gz');a=parents(events);d=rq.requests(events);assert not a['issues'] and not d['issues'],(a['issues'],d['issues'])
  start=r['schedule']['origin_ns'];end=r['schedule']['drained_ns'];sync=rq.base.analyze(p/'trace.txt.gz',start,end)
  assert not sync['issues'] and len(sync['syncs'])==r['sync_attempts']
  cs=[c for c in a['callbacks'] if start<=c['entry_ns']<=end];members=[m for c in cs for m in c['members']]
  summary=dict(case=r['kernel_trace'],valid=True,callbacks=len(cs),members=len(members),journal_members=sum(bool(m['parent'] and m['parent']['journal']) for m in members),unknown_members=sum(not(m['parent'] and m['parent']['journal']) for m in members),shared_callbacks=sum(len(c['members'])>1 for c in cs),empty_callbacks=sum(not c['members'] for c in cs),missing_dispatch=sum(c['dispatch'] is None for c in cs),syncs=len(sync['syncs']))
  summary['max_dispatch_to_callback_ms']=max((c['entry_ns']-c['dispatch'][1])/1e6 for c in cs if c['dispatch']) if cs else None
  groups=collections.defaultdict(list)
  for c in cs:groups[(c['flush'],tuple(c['dispatch']) if c['dispatch'] else None)].extend(c['members'])
  summary['unresolved_dispatches']=sum(not ms for ms in groups.values())
  summary['gate']=bool(cs) and not(summary['unknown_members'] or summary['missing_dispatch'] or summary['unresolved_dispatches'])
  journals=ja.journals(events,start,end);assert not journals['issues'],journals['issues']
  summary['worst_sync']=max(sync['syncs'],key=lambda x:x['duration_ns'])
  summary['worst_journal']=max(journals['transactions'],key=lambda x:x['duration_ns'])
  summary['worst_flush']=max((c for c in cs if c['members'] and c['dispatch']),key=lambda c:c['entry_ns']-c['dispatch'][1])
  summary['max_journal_runnable_ns']=max(x['runnable_ns'] for x in journals['transactions'])
  worst=summary['worst_journal'];attached=[]
  for c in cs:
   if any(m['parent'] and m['parent']['journal']==dict(device=worst['device'],transaction=worst['transaction']) for m in c['members']):attached.append(c)
  summary['worst_journal_flushes']=attached
  summaries.append(summary);details.append(dict(case=summary['case'],callbacks=cs))
 with gzip.open(root/'flush-analysis.json.gz','wt') as f:json.dump(details,f)
 (root/'flush-summary.json').write_text(json.dumps(summaries,indent=2));print(json.dumps(summaries,indent=2))
if __name__=='__main__':main(sys.argv[1])
