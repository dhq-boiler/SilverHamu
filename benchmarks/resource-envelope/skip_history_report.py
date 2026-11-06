"""Validate and compare inline-latest trials without mixing diagnostic latency into service results."""
import argparse,collections,gzip,json,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
p=argparse.ArgumentParser();p.add_argument('--input',type=pathlib.Path,required=True);a=p.parse_args();out=a.input;raw=out/'raw';M=2**20;med=statistics.median
def text(p):
 return p.read_text(encoding='utf8') if p.exists() else gzip.open(str(p)+'.gz','rt',encoding='utf8').read()
def js(p):return json.loads(text(p))
def lines(p):return [json.loads(l) for l in text(p).splitlines()]
cases=[];checks=0;service=[];totals=collections.Counter()
for variant in ['before','after','skip']:
 for rep in range(1,4):
  root=raw/f'memory-{variant}-{rep}';entries=js(root/'results.json');assert len(entries)==2
  for c in entries:
   assert c['status']=='passed';snaps={};events=lines(root/c['name']/'aging.jsonl');resources={}
   for label,state in c['phases'].items():
    samples=lines(root/c['name']/(label+'-resources.jsonl'));es=lines(root/c['name']/(label+'.jsonl'))
    assert all(s['swap']==0 for s in samples) and all(x['time_ns']<=y['time_ns'] for x,y in zip(samples,samples[1:]));assert not state['exit']['timed_out']
    if not state['success']:assert label=='aged512' and state['oom_kills']>0
    else:assert any(e['event']=='verified' and e['rows']==1000000 for e in es)
    checks+=len(state['checks']);resources[label]={'success':state['success'],'oom_kills':state['oom_kills'],'peak_mib':max(s['peak'] for s in samples)/M,'rss_peak_mib':max(s['rss'] for s in samples)/M,'file_peak_mib':max(s['stat']['file'] for s in samples)/M,'open_ms':next((e['ns']/1e6 for e in es if e['event']=='opened'),None)}
   for e in events:
    if e['event']!='snapshot':continue
    m=e['metrics'];assert m['records']==1000000 and m['versions']>=1000000 and m['capacity_slots']>=m['versions'];assert m['row_size']==32 and m['version_size']==24
    assert e['logical_retained_bytes']==m['records']*208+m['versions']*320
    assert m['inline_versions']==(1000000 if variant!='before' else 0)
    if m['versions']==1000000 and variant!='before':assert m['heap_history_bytes']==0
    snaps[e['phase']]=e
   assert snaps['before-trim']['metrics']==snaps['after-trim']['metrics'];assert snaps['before-trim']['live_requested_bytes']==snaps['after-trim']['live_requested_bytes']
   if variant!='skip': assert all(e['metrics']['skipped_history_versions']==0 for e in snaps.values())
   elif c['pattern']=='spread': assert 128000<=snaps['spread-100']['metrics']['skipped_history_versions']<=128800
   else: assert snaps['hot-held']['metrics']['skipped_history_versions']==0
   gc=None
   if c['pattern']=='hot':
    assert snaps['hot-held']['metrics']['versions']==1160000 and snaps['hot-drained']['metrics']['versions']==1000000
    idx=next(i for i,e in enumerate(events) if e['event']=='release');gc=(events[idx]['ns']+next(e['ns'] for e in events[idx+1:] if e['event']=='drain'))/1e6
   cases.append({'variant':variant,'rep':rep,'pattern':c['pattern'],'snapshots':snaps,'resources':resources,'release_and_drain_ms':gc})
  for cohort,pin in [('service',True),('service-unpinned',False)]:
   root=raw/f'{cohort}-{variant}-{rep}';cs=js(root/'results.json');assert len(cs)==1;c=cs[0];assert c['status']=='passed';assert len(c['verification'])==4 and all(v['passed'] and v['rows']==1000000 for v in c['verification'])
   for m in c['measurements']:
    assert m['pin_reader']==pin
    assert m['old_snapshot_checked_keys']==(m['by_kind'].get('write',{}).get('completed',0) if pin else 0)
    records=js(root/c['name']/(m['work']+'-raw.json'))['records'];assert len(records)==m['ops']==8000 and len({r['seq'] for r in records})==8000
    assert all(r['planned']<=r['enqueued']<=r['ended'] for r in records)
    assert all(r['enqueued']<=r['began']<=r['ended'] for r in records if r['status']!='queue_full')
    assert m['completed']+m['queue_rejected']+len(m['errors'])==8000
    totals['requests']+=8000;totals['rejected']+=m['queue_rejected'];totals['errors']+=len(m['errors'])
   checks+=len(c['verification']);service.append({**c,'variant':variant,'rep':rep,'pin_reader':pin})
totals['full_checks']=checks
summary={'totals':dict(totals),'memory':cases,'service':service};(out/'summary.json').write_text(json.dumps(summary,indent=2),encoding='utf8')
def snapshots(v,pattern,phase):return [c['snapshots'][phase] for c in cases if c['variant']==v and c['pattern']==pattern]
table=['| Pattern / phase | Mode | Live requested MiB | RSS MiB | History heap MiB | Skipped history versions |','|:--|:--|--:|--:|--:|--:|']
for pattern,phases in [('spread',['initial','spread-25','spread-50','spread-100','after-trim']),('hot',['initial','hot-held','hot-drained','before-trim','after-trim'])]:
 for phase in phases:
  for v in ['before','after','skip']:
   ss=snapshots(v,pattern,phase);table.append(f'| {pattern}/{phase} | {v} | {med(s["live_requested_bytes"]/M for s in ss):.3f} | {med(s["rss_bytes"]/M for s in ss):.3f} | {med(s["metrics"]["heap_history_bytes"]/M for s in ss):.3f} | {med(s["metrics"]["skipped_history_versions"] for s in ss):.0f} |')
(out/'memory-table.md').write_text('\n'.join(table)+'\n')
table=['| Mode / trial / old reader pinned | Read p99 ms | Mixed read p99 ms | Mixed write p99 ms | Mixed max ms | Mixed ops/s | Rejected |','|:--|--:|--:|--:|--:|--:|--:|']
for c in service:
 read=next(m for m in c['measurements'] if m['work']=='read');mix=next(m for m in c['measurements'] if m['work']=='mixed')
 table.append(f'| {c["variant"]}/{c["rep"]}/{c["pin_reader"]} | {read["p99_ms"]:.3f} | {mix["by_kind"]["read"]["p99_ms"]:.3f} | {mix["by_kind"]["write"]["p99_ms"]:.3f} | {mix["max_ms"]:.3f} | {mix["ops_s"]:.1f} | {read["queue_rejected"]+mix["queue_rejected"]} |')
(out/'service-table.md').write_text('\n'.join(table)+'\n')
colors=['#777777','#226aa8','#b36b19'];labels=['Deque baseline','Inline latest','Skip unneeded history']
fig,axes=plt.subplots(1,3,figsize=(14,4.5))
for vi,v in enumerate(['before','after','skip']):
 for ax,field,title in [(axes[0],'live_requested_bytes','Live requested allocation'),(axes[1],'rss_bytes','Process RSS')]:
  phases=['initial','spread-25','spread-50','spread-100'];vals=[[s[field]/M for s in snapshots(v,'spread',ph)] for ph in phases]
  ax.plot(range(4),[med(x) for x in vals],marker=['o','s','^'][vi],linestyle=['-','--',':'][vi],color=colors[vi],label=labels[vi]);ax.set_xticks(range(4),['Initial','25%','50%','100%']);ax.set_title(title);ax.set_ylabel('MiB (nonzero origin)')
  for i,ys in enumerate(vals):ax.scatter([i]*3,ys,color=colors[vi],s=10)
 vals=[c['release_and_drain_ms'] for c in cases if c['variant']==v and c['pattern']=='hot'];axes[2].bar(vi,med(vals),color=colors[vi]);axes[2].scatter([vi]*3,vals,c='black',s=15,zorder=3)
axes[2].set_xticks([0,1,2],['Deque','Inline','Skip']);axes[2].set_title('Hot 1%: release + remaining GC');axes[2].set_ylabel('ms (diagnostic build)')
axes[0].legend(fontsize=8)
for ax in axes:ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
fig.suptitle('1,000,000 rows / 256 B / 1 CPU / 1 GiB / 3 counterbalanced rounds\nSpread updates with GC drained; dots are individual trials',fontsize=11);fig.tight_layout();fig.savefig(out/'memory.svg');fig.savefig(out/'memory.png',dpi=140);plt.close(fig)
for pin,label in [(True,'pinned'),(False,'unpinned')]:
 fig,axes=plt.subplots(1,3,figsize=(14,4.5))
 for vi,v in enumerate(['before','after','skip']):
  cs=[c for c in service if c['variant']==v and c['pin_reader']==pin];rs=[next(m for m in c['measurements'] if m['work']=='read') for c in cs];ms=[next(m for m in c['measurements'] if m['work']=='mixed') for c in cs]
  for ax,vals,title in [(axes[0],[m['p99_ms'] for m in rs],'Read-only p99'),(axes[1],[m['by_kind']['write']['p99_ms'] for m in ms],'Mixed write p99'),(axes[2],[m['max_ms'] for m in ms],'Mixed maximum response')]:
   ax.bar(vi,med(vals),color=colors[vi]);ax.scatter([vi+(i-1)*.07 for i in range(3)],vals,c='black',s=16,zorder=3);ax.set_title(title);ax.set_ylabel('ms; scheduled arrival to completion');ax.set_xticks([0,1,2],['Deque','Inline','Skip']);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
 fig.suptitle(('Old reader pinned: '+str(pin)+' / Uninstrumented service /  1,000,000 rows / 1 CPU / 1 GiB\n8,000 requests at 1,000/s; mixed = 1 reader + 4 writers; medians and all 3 trials'),fontsize=11);fig.tight_layout();fig.savefig(out/f'service-{label}.svg');fig.savefig(out/f'service-{label}.png',dpi=140);plt.close(fig)
for f in out.glob('*.svg'):f.write_text('\n'.join(l.rstrip() for l in f.read_text().splitlines())+'\n')
print(json.dumps(totals));print('\n'.join(table))
