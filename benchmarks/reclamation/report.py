"""Recompute native percentiles from raw per-thread samples and retain all runs."""
import gzip,json,math,pathlib,statistics,sys
p=pathlib.Path(sys.argv[1]); rows=json.loads((p/'results.json').read_text(encoding='utf-8'));assert len(rows)==36
computed=[]
for r in rows:
 with gzip.open(p/r['raw_file'],'rt',encoding='utf-8') as f:raw=json.load(f)
 assert raw['verified'] and raw['scenario']==r['scenario']
 c={k:r[k] for k in ('variant','scenario','rep')}
 for group in ('readers','writers'):
  samples=[]
  for thread in raw[group]:
   ns=thread['latency']['samples_ns'];assert len(ns)==thread['latency']['count'];assert ns==sorted(ns)
   assert ns[-1]/1e6==thread['latency']['max_ms'];assert ns[math.ceil(len(ns)*.99)-1]/1e6==thread['latency']['p99_ms']
   samples+=ns
  samples.sort();c[group+'_count']=len(samples)
  c[group+'_p99_ms']=samples[math.ceil(len(samples)*.99)-1]/1e6 if samples else 0
  c[group+'_max_ms']=samples[-1]/1e6 if samples else 0
 c['write_ops_s']=c['writers_count']/r['elapsed_s']
 c['hold_max_ms']=max([r['main_hold'][2],r['worker_max_lock_hold_ns']]+[t['hold'][2] for g in ('readers','writers') for t in r[g]])/1e6
 c['drop_ms']=r['drop_ns']/1e6;c['drain_ms']=r['drain_ns']/1e6;c['total_gc_ms']=c['drop_ms']+c['drain_ms']
 for metric in ('heap_start','heap_peak','heap_end','heap_drained'):c[metric+'_mib']=r[metric]/1048576
 c['heap_extra_mib']=(r['heap_peak']-r['heap_start'])/1048576
 c['retained_drained_mib']=r['budget_drained'][3]/1048576
 c['retained_current_mib']=r['budget_current'][3]/1048576;c['retained_peak_mib']=r['budget_peak'][3]/1048576
 computed.append(c)
(p/'computed.json').write_text(json.dumps(computed,indent=2),encoding='utf-8')
metrics=['readers_p99_ms','readers_max_ms','writers_p99_ms','write_ops_s','hold_max_ms','drop_ms','total_gc_ms','heap_peak_mib','heap_drained_mib','retained_drained_mib']
lines=['| Scenario | Variant | Read p99 ms | Read max ms | Write p99 ms | Write ops/s | Max lock hold ms | Drop ms | Drop + drain ms | Peak heap MiB | Drained heap MiB | Drained retained MiB |','|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
for scenario in ('mixed','large','gc'):
 for variant in ('cooperative','background','cooperative-light','background-light'):
  group=[r for r in computed if r['scenario']==scenario and r['variant']==variant];assert len(group)==3
  lines.append('| '+scenario+' | '+variant+' | '+' | '.join(f'{statistics.median(r[m] for r in group):.3f}' for m in metrics)+' |')
(p/'table.md').write_text('Median of three run-level values (maxima are medians of per-run maxima).\n\n'+'\n'.join(lines)+'\n',encoding='utf-8')
print('Validated 36 native runs; all raw percentiles recomputed.')
