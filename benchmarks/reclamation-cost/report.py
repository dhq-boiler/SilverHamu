import pathlib,json,gzip,math,statistics,sys,itertools
p=pathlib.Path(sys.argv[1]);rows=json.loads((p/'results.json').read_text(encoding='utf-8'));env=json.loads((p/'environment.json').read_text(encoding='utf-8'))
variants=list(env['images']);scenarios=env['scenarios'];expected=set(itertools.product(variants,scenarios,range(1,4)))
assert len(rows)==len(expected) and {(r['variant'],r['scenario'],r['rep']) for r in rows}==expected
computed=[]
for r in rows:
 with gzip.open(p/r['raw_file'],'rt',encoding='utf-8') as f:raw=json.load(f)
 assert raw['verified'] and all(raw[k]==r[k] for k in ('scenario','variant','rep'))
 assert raw['budget_drained']==[0,0,0,raw['rows']*(272+raw['value_bytes']),0]
 c={k:r[k] for k in ('variant','scenario','rep')}
 for group in ('readers','writers'):
  ns=[]
  for t in raw[group]:
   x=t['latency']['samples_ns'];assert len(x)==t['latency']['count'];assert x==sorted(x)
   assert x[-1]/1e6==t['latency']['max_ms'];assert x[math.ceil(len(x)*.99)-1]/1e6==t['latency']['p99_ms'];ns+=x
  ns.sort();c[group+'_count']=len(ns);c[group+'_p99_ms']=ns[math.ceil(len(ns)*.99)-1]/1e6 if ns else 0;c[group+'_max_ms']=ns[-1]/1e6 if ns else 0
 assert c['writers_count']==(0 if r['scenario']=='gc' else 400 if r['scenario']=='large' else 1200)
 c['write_ops_s']=c['writers_count']/r['elapsed_s'];c['total_gc_ms']=(r['drop_ns']+r['drain_ns'])/1e6
 c['hold_max_ms']=max([r['main_hold'][2],r['worker_max_lock_hold_ns']]+[t['hold'][2] for g in ('readers','writers') for t in r[g]])/1e6
 for k in ('heap_start','heap_peak','heap_drained'):c[k+'_mib']=r[k]/1048576
 c['retained_drained_mib']=r['budget_drained'][3]/1048576
 profile=r['worker_profile'];measured=sum(profile[k] for k in ('work_ns','free_ns','yield_ns','pause_elapsed_ns'));assert measured<=profile['worker_elapsed_ns']
 if r['lock_timing'] and r['background']:assert profile['worker_elapsed_ns']>0
 else:assert all(v==0 for v in profile.values())
 for k,v in profile.items():c[k[:-3]+'_ms' if k.endswith('_ns') else k]=v/1e6 if k.endswith('_ns') else v
 if r['scenario']=='gc' and r['background']:assert r['worker_reclaimed_bytes']==32_000_000
 computed.append(c)
(p/'computed.json').write_text(json.dumps(computed,indent=2)+'\n',encoding='utf-8')
metrics=['readers_p99_ms','readers_max_ms','writers_p99_ms','write_ops_s','total_gc_ms','hold_max_ms','heap_peak_mib','heap_drained_mib','retained_drained_mib']
lines=['| Scenario | Variant | Read p99 ms | Worst read across repeats ms | Write p99 ms | Write ops/s | Complete GC ms | Max hold ms | Peak heap MiB | Drained heap MiB | Drained retained MiB |','|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
for scenario in scenarios:
 for variant in variants:
  group=[r for r in computed if r['scenario']==scenario and r['variant']==variant]
  lines.append('| '+scenario+' | '+variant+' | '+' | '.join(f'{(max if m=="readers_max_ms" else statistics.median)(r[m] for r in group):.3f}' for m in metrics)+' |')
(p/'table.md').write_text('Medians of three runs, except worst read is the maximum across all repeats.\n\n'+'\n'.join(lines)+'\n',encoding='utf-8')
print('Validated',len(rows),'native runs, raw percentiles, complete drain and timing accounting.')
