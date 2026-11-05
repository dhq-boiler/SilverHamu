"""Validate per-worker correspondence before attributing service outliers."""
import gzip,importlib.util,json,math,statistics as st,sys,shutil
from pathlib import Path

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('commit_phases',HERE.parent/'commit-breakdown/report.py')
phase_module=importlib.util.module_from_spec(spec);spec.loader.exec_module(phase_module)

def quantile(xs,p):
 xs=sorted(xs);return xs[max(0,math.ceil(len(xs)*p)-1)]

def validate_run(run,latencies,raw):
 assert len(latencies)==run['completed']==run['attempted'] and not run['errors']
 assert raw['dropped']==0 and raw['limit']==run['attempted']
 records=raw['records'];assert len(records)<=raw['limit']
 seen=set();resolved=[]
 for row in records:
  key=(row['worker'],row['sequence']);assert key not in seen;seen.add(key)
  assert 0<=row['worker']<run['clients'] and 0<=row['sequence']<run['attempted']//run['clients']
  index=row['worker']*(run['attempted']//run['clients'])+row['sequence']
  assert latencies[index]==row['operation_ns']
  assert not row.get('probe_error') and row['diagnostic']['commit_ok']
  assert row['request_index']==row['diagnostic']['request_index']
  phases=phase_module.phases(row)
  phases['outside protocol request']=row['operation_ns']-row['request_ns']
  assert phases['outside protocol request']>=0 and sum(phases.values())==row['operation_ns']
  resolved.append({**row,'phases':phases,'largest_phase':max(phases,key=phases.get),
                   'sync_fraction':phases['log sync']/row['operation_ns']})
 for worker in range(run['clients']):
  subset=latencies[worker*(run['attempted']//run['clients']):(worker+1)*(run['attempted']//run['clients'])]
  assert max(row['operation_ns'] for row in records if row['worker']==worker)==max(subset)
 for threshold in [10,20,50]:
  assert sum(x>threshold*1_000_000 for x in latencies)==sum(row['operation_ns']>threshold*1_000_000 for row in records)
 return resolved

def main():
 source=Path(sys.argv[1]);out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
 rows=json.loads((source/'results.json').read_text());environment=json.loads((source/'environment.json').read_text())
 assert len(rows)==40 and len(environment['runs'])==20
 assert all(x['status']=='passed' and len(x['checks'])==4 for x in environment['runs'])
 for f in source.iterdir():
  if f.is_file():shutil.copy2(f,out/f.name)
 trials=[];tails=[]
 for r in rows:
  latency=json.load(gzip.open(source/r['latencies_ns_file']))
  assert len(latency)==8000 and r['completed']==8000 and not r['errors']
  item={k:r[k] for k in ['variant','clients','rep','ops_s','p50_ms','p95_ms','p99_ms','max_ms','elapsed_s']}
  item['p999_ms']=quantile(latency,.999)/1e6
  item['above_ms']={str(t):sum(x>t*1e6 for x in latency) for t in [10,20,50]}
  if 'profile_file' in r:
   raw=json.load(gzip.open(source/r['profile_file']));records=validate_run(r,latency,raw)
   maximum=max(records,key=lambda x:x['operation_ns'])
   selected=[x for x in records if x['operation_ns']>10_000_000]
   item['probe_count']=len(records);item['probe_elapsed_sum_ms']=sum(x['probe_ns'] for x in records)/1e6
   item['max_record']=maximum
   item['tail_classification']={name:sum(x['largest_phase']==name for x in selected) for name in sorted({x['largest_phase'] for x in selected})}
   item['sync_majority_count']=sum(x['sync_fraction']>=.5 for x in selected)
   for x in records:tails.append({'variant':r['variant'],'clients':r['clients'],'rep':r['rep'],**x})
  trials.append(item)
 variants=['before','candidate','before-light','candidate-light'];summary=[]
 for c in [1,4]:
  for v in variants:
   group=[t for t in trials if t['variant']==v and t['clients']==c];assert len(group)==5
   x={'variant':v,'clients':c,'operations':40000,'runs':5,'above_ms':{str(n):sum(t['above_ms'][str(n)] for t in group) for n in [10,20,50]}}
   for metric in ['ops_s','p50_ms','p99_ms','p999_ms','max_ms']:
    vals=[t[metric] for t in group];x[metric]={'median':st.median(vals),'min':min(vals),'max':max(vals)}
   summary.append(x)
 (out/'summary.json').write_text(json.dumps({'summary':summary,'trials':trials},indent=2)+'\n',encoding='utf-8')
 with gzip.open(out/'attributed-records.json.gz','wt',encoding='utf-8') as f:json.dump(tails,f)
 table=['| Connections | Mode | Median updates/s | Median p99 ms | Median run maximum ms | Worst maximum ms | >10ms / 40k | >20ms / 40k | >50ms / 40k |',
        '|---:|---|---:|---:|---:|---:|---:|---:|---:|']
 for x in summary:
  table.append(f"| {x['clients']} | {x['variant']} | {x['ops_s']['median']:.1f} | {x['p99_ms']['median']:.3f} | {x['max_ms']['median']:.3f} | {x['max_ms']['max']:.3f} | "+' | '.join(str(x['above_ms'][str(n)]) for n in [10,20,50])+' |')
 (out/'table.md').write_text('\n'.join(table)+'\n',encoding='utf-8')
 import matplotlib
 matplotlib.use('Agg')
 import matplotlib.pyplot as plt
 colors=['#667085','#007f85']
 fig,axes=plt.subplots(2,2,figsize=(12,8),layout='constrained')
 for ax,metric,title in zip(axes.flat,['ops_s','p99_ms','max_ms','over10'],['Updates / second','Per-run p99 (ms)','Per-run maximum (ms)','Requests exceeding 10 ms / 8,000']):
  for j,v in enumerate(variants[:2]):
   for i,c in enumerate([1,4]):
    group=[t for t in trials if t['variant']==v and t['clients']==c]
    vals=[t[metric] if metric!='over10' else t['above_ms']['10'] for t in group]
    xpos=i+(j-.5)*.3;median=st.median(vals)
    ax.bar(xpos,median,.28,color=colors[j],alpha=.6,label=v if i==0 else None)
    ax.scatter([xpos+(k-2)*.025 for k in range(5)],vals,color=colors[j],edgecolors='black',linewidth=.4,s=23,zorder=3)
   ax.set_xticks([0,1],['1 connection','4 connections']);ax.set_title(title);ax.spines[['top','right']].set_visible(False)
 axes[0,0].legend();fig.suptitle('Maximum-latency recheck: five trials per mode (instrumentation OFF)')
 fig.supxlabel('Bars: median; dots: every trial, including adverse results. 100k rows, 256B values, 8,000 updates/trial.\nTwo CPUs / 4 GiB per container; warm-cache Python TCP clients. No retries or outlier removal.',fontsize=9)
 for ext in ['svg','png']:fig.savefig(out/f'service.{ext}',dpi=140)
 plt.close(fig)
 fig,axes=plt.subplots(1,2,figsize=(13,6),layout='constrained')
 buckets={'Log sync':['log sync'],'Collection + queue':['leader queue','batch collection','writer lock','state lock','publication lock'],
          'Other server':['dispatch preparation','queue submission','selection overhead','validation / encoding / bookkeeping','log reservation','log write','publication','cleanup / replies ahead','reply wake / leader exit','dispatch finish'],
          'Transport / client residual':['transport / client residual','outside protocol request']}
 palette=['#1b6d85','#e0a438','#479e83','#c7c7c7']
 for ax,c in zip(axes,[1,4]):
  group=[next(t for t in trials if t['clients']==c and t['rep']==rep and t['variant']==v) for rep in range(1,6) for v in ['before-light','candidate-light']]
  bottom=[0]*10
  for (name,names),color in zip(buckets.items(),palette):
   vals=[sum(t['max_record']['phases'][n] for n in names)/1e6 for t in group]
   ax.bar(range(10),vals,bottom=bottom,color=color,label=name);bottom=[x+y for x,y in zip(bottom,vals)]
  ax.set_xticks(range(10),[('B' if t['variant']=='before-light' else 'C')+str(t['rep']) for t in group]);ax.set_ylabel('Maximum operation time (ms)');ax.set_title(f'{c} connection(s)');ax.spines[['top','right']].set_visible(False)
 axes[0].legend(loc='upper left',fontsize=9)
 fig.suptitle('Actual worst request in each lightweight-instrumented trial')
 fig.supxlabel('B = before; C = candidate. One bounded record per connection; fetched after the timed operation.\nElapsed intervals do not distinguish device I/O from scheduling inside the sync call.',fontsize=9)
 for ext in ['svg','png']:fig.savefig(out/f'tail-breakdown.{ext}',dpi=140)
 plt.close(fig)
 for p in out.glob('*.svg'):p.write_text('\n'.join(line.rstrip() for line in p.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
 print('\n'.join(table))

if __name__=='__main__':main()
