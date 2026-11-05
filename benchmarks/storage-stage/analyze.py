"""Follow driver requests; do not label the host interval as disk service time."""
import collections,gzip,importlib.util,json,pathlib,re,sys
spec=importlib.util.spec_from_file_location('flush',pathlib.Path(__file__).resolve().parent.parent/'flush-parent/analyze.py');fp=importlib.util.module_from_spec(spec);spec.loader.exec_module(fp)
def stages(events):
 generations=collections.Counter();active={};rows={};channels=collections.defaultdict(list);pending=collections.defaultdict(list);issues=[]
 for e in events:
  b=e['body'];kind=b.split(':')[0];t=e['ns'];ctx=(e['cpu'],e['pid'])
  if kind not in ('rq_start','rq_requeue','st_queue','st_queue_return','st_done','st_channel','st_channel_return'):continue
  slot=re.search(r'rq=(R\d+)',b)[1]
  if kind=='rq_start':
   generations[slot]+=1;a=dict(slot=slot,attempt=generations[slot],start_ns=t);active[slot]=a;rows[(slot,a['attempt'],t)]=a
  elif kind=='st_channel':channels[ctx].append((slot,t))
  elif kind=='st_channel_return':
   if not channels[ctx] or channels[ctx].pop()[0]!=slot:issues.append('channel return mismatch')
  elif kind=='st_queue':
   a=active.get(slot)
   if a and 'queue_ns' in a:issues.append('duplicate submission')
   if a:a['queue_ns']=t
   pending[(slot,e['pid'])].append(a)
  elif kind=='st_queue_return':
   stack=pending[(slot,e['pid'])]
   if not stack:issues.append('submission return without entry');continue
   a=stack.pop()
   if a:a['queue_return_ns']=t;a['queue_result']=int(re.search(r'result=(-?\d+)',b)[1])
  elif kind=='st_done':
   a=active.get(slot)
   if a:
    if 'done_ns' in a:issues.append('duplicate driver completion')
    a['done_ns']=t;a['channel_entry_ns']=channels[ctx][-1][1] if channels[ctx] else None
  elif kind=='rq_requeue' and slot in active:active[slot]['requeued']=True
 if any(channels.values()) or any(pending.values()):issues.append('incomplete call')
 return rows,issues

def join(events,start,end):
 a=fp.parents(events);rows,issues=stages(events);issues+=a['issues'];out=[];seen=set()
 for c in a['callbacks']:
  if not start<=c['entry_ns']<=end or not c['members']:continue
  key=(c['flush'],*(c['dispatch'] or (None,None)))
  if key in seen:issues.append('duplicate member-bearing completion');continue
  seen.add(key);r=rows.get(key)
  if not r or any(k not in r for k in ('queue_ns','queue_return_ns','queue_result','done_ns')) or r['channel_entry_ns'] is None:
   issues.append('unmatched driver stage');continue
  if r['queue_result']!=0 or r.get('requeued'):issues.append('retry or submission failure');continue
  if not r['start_ns']<=r['queue_ns']<=r['done_ns']<=c['entry_ns'] or r['queue_return_ns']<r['queue_ns']:issues.append('invalid time order');continue
  if any(not(m['parent'] and m['parent']['journal']) for m in c['members']):issues.append('unknown journal');continue
  out.append(dict(**r,flush_callback_ns=c['entry_ns'],pre_driver_ns=r['queue_ns']-r['start_ns'],driver_to_done_ns=r['done_ns']-r['queue_ns'],after_done_ns=c['entry_ns']-r['done_ns'],total_ns=c['entry_ns']-r['start_ns'],members=c['members']))
 return out,issues

def main(folder):
 root=pathlib.Path(folder);summaries=[];details=[]
 for r in json.loads((root/'results.json').read_text()):
  if 'kernel_trace' not in r:continue
  p=root/r['kernel_trace'];m=json.loads((p/'metadata.json').read_text());errors=[]
  if m['request_probe_count']!=15 or len(m['probe_profile'])!=15 or any(int(x.split()[-1]) for x in m['probe_profile']):errors.append('missing or missed probes')
  errors+=fp.quality(dict(m,request_probe_count=10,probe_profile=m['probe_profile'][:10]))
  if errors:summaries.append(dict(case=r['kernel_trace'],valid=False,issues=sorted(set(errors))));continue
  events=fp.rq.base.parse(p/'trace.txt.gz');start=r['schedule']['origin_ns'];end=r['schedule']['drained_ns'];out,issues=join(events,start,end)
  sync=fp.rq.base.analyze(p/'trace.txt.gz',start,end);issues+=sync['issues']
  if len(sync['syncs'])!=r['sync_attempts']:issues.append('sync counter mismatch')
  s=dict(case=r['kernel_trace'],valid=bool(out) and not issues,issues=issues,flushes=len(out))
  if out:s.update(worst=max(out,key=lambda x:x['total_ns']),max_after_done_ns=max(x['after_done_ns'] for x in out),max_pre_driver_ns=max(x['pre_driver_ns'] for x in out))
  summaries.append(s);details.append(dict(case=s['case'],rows=out))
 (root/'storage-summary.json').write_text(json.dumps(summaries,indent=2))
 with gzip.open(root/'storage-analysis.json.gz','wt') as f:json.dump(details,f)
 print(json.dumps(summaries,indent=2))
if __name__=='__main__':main(sys.argv[1])
