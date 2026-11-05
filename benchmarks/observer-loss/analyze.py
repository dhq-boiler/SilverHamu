"""Keep probe misses and ring loss separate; never waive either quality check."""
import collections,gzip,importlib.util,json,pathlib,re,sys
spec=importlib.util.spec_from_file_location('storage',pathlib.Path(__file__).resolve().parent.parent/'storage-stage/analyze.py');storage=importlib.util.module_from_spec(spec);spec.loader.exec_module(storage)
FULL=set('rq_start rq_end rq_update rq_free rq_requeue rq_update_return fp_insert fp_seq fp_end fp_return st_queue st_queue_return st_done st_channel st_channel_return'.split())
def profile(lines):
 out={}
 for line in lines:
  name,hits,misses=line.split()
  if name in out or min(int(hits),int(misses))<0:raise ValueError('invalid profile')
  out[name]=dict(hits=int(hits),misses=int(misses))
 return out
def quality(m):
 mode=m.get('mode');names=FULL if mode in ('full','capacity') else FULL-{'rq_end','rq_update','rq_update_return'} if mode=='minimal' else set() if mode=='standard' else None
 errors=[];p=profile(m['probe_profile']);before=profile(m['profile_before'])
 if names is None or set(p)!=names or set(before)!=names or m['request_probe_count']!=len(names or []):errors.append('probe set mismatch')
 if any(x['misses'] for x in p.values()):errors.append('missed probes')
 if any(n not in p or p[n]['misses']<v['misses'] or p[n]['hits']<v['hits'] for n,v in before.items()):errors.append('counter regression')
 if not m.get('cleaned'):errors.append('incomplete cleanup')
 if not m.get('stats'):errors.append('missing ring counters')
 for text in m.get('stats',{}).values():
  for field in ['overrun','commit overrun','dropped events']:
   x=re.search('^'+field+r': (\d+)',text,re.M)
   if not x or int(x[1]):errors.append('ring loss or missing counters')
 return sorted(set(errors))
def main(folder):
 root=pathlib.Path(folder);out=[]
 for r in json.loads((root/'results.json').read_text()):
  if 'kernel_trace' not in r:continue
  p=root/r['kernel_trace'];m=json.loads((p/'metadata.json').read_text());errors=quality(m);prof=profile(m['probe_profile']);before=profile(m['profile_before'])
  events=storage.fp.rq.base.parse(p/'trace.txt.gz');start=r['schedule']['origin_ns'];end=r['schedule']['drained_ns']
  s=dict(case=r['kernel_trace'],mode=m['mode'],quality_errors=list(errors),valid_capture=not errors,gate=False,trace_bytes=(p/'trace.txt.gz').stat().st_size,event_count=len(events),misses={n:x['misses'] for n,x in prof.items() if x['misses']},before_misses={n:x['misses'] for n,x in before.items() if x['misses']},extra_return_misses={})
  for entry,ret in [('rq_update','rq_update_return'),('fp_end','fp_return'),('st_queue','st_queue_return'),('st_channel','st_channel_return')]:
   if ret in prof and entry in prof:s['extra_return_misses'][ret]=prof[ret]['misses']-prof[entry]['misses']
  if not errors:
   sync=storage.fp.rq.base.analyze(p/'trace.txt.gz',start,end);errors.extend(sync['issues'])
   if len(sync['syncs'])!=r['sync_attempts']:errors.append('sync count mismatch')
   if m['mode']=='standard':s['limitation']='No direct request-pointer and journal-parent mapping; not eligible for causal comparison'
   else:
    rows,issues=storage.join(events,start,end);errors.extend(issues);s['matched_flushes']=len(rows);s['gate']=bool(rows) and not errors
  s['analysis_errors']=sorted(set(errors));out.append(s)
 (root/'observer-summary.json').write_text(json.dumps(out,indent=2));print(json.dumps(out,indent=2))
if __name__=='__main__':main(sys.argv[1])

