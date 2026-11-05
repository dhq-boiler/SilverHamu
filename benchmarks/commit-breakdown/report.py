"""Validate bounded per-request traces and render the paired experiment."""
import gzip,json,math,statistics as stats,sys,shutil
from pathlib import Path

def percentile(xs,p):
 xs=sorted(xs);return xs[max(0,math.ceil(len(xs)*p)-1)]

def phases(row):
 d=row['diagnostic'];t=d['trace'];q=d['detail'];cost=d['costs']
 assert row['ok'] and len(t)==8 and len(q)==8 and len(cost)==2
 assert all(x>0 for x in t+q) and t==sorted(t)
 start=d['finished_ns']-d['dispatch_ns']
 pairs=[('dispatch preparation',start,q[7]),('queue submission',q[7],t[0]),
        ('leader queue',t[0],max(q[1],t[0])),('batch collection',max(q[1],t[0]),t[1]),
        ('selection overhead',t[1],t[2]),('writer lock',t[2],t[3]),
        ('state lock',t[3],q[2]),('validation / encoding / bookkeeping',q[2],q[3]),
        ('log reservation',q[3],t[4]),('log write',t[4],t[5]),('log sync',t[5],t[6]),
        ('publication lock',t[6],q[4]),('publication',q[4],t[7]),
        ('cleanup / replies ahead',t[7],q[5]),('reply wake / leader exit',q[5],q[6]),
        ('dispatch finish',q[6],d['finished_ns'])]
 assert q[0]>=q[7] and q[0]<=t[0] and q[1]<=t[1]
 assert all(end>=begin for _,begin,end in pairs)
 assert sum(cost)<=q[3]-q[2]
 result={name:end-begin for name,begin,end in pairs}
 assert sum(result.values())==d['dispatch_ns']
 result['transport / client residual']=row['request_ns']-d['dispatch_ns']
 assert result['transport / client residual']>=0
 assert sum(result.values())==row['request_ns']
 return result

def main():
 source=Path(sys.argv[1]);out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
 rows=json.loads((source/'results.json').read_text());env=json.loads((source/'environment.json').read_text())
 assert len(rows)==24 and len(env['runs'])==12
 assert all(r['status']=='passed' and len(r['checks'])==4 for r in env['runs'])
 assert all(r['attempted']==8000 and r['completed']==8000 and not r['errors'] for r in rows)
 for f in source.iterdir():
  if f.is_file():shutil.copy2(f,out/f.name)
 variants=['before','candidate','before-diag','candidate-diag'];summary=[];detail=[]
 for clients in [1,4]:
  for v in variants:
   runs=[r for r in rows if r['clients']==clients and r['variant']==v];assert len(runs)==3
   item={'variant':v,'clients':clients}
   for metric in ['ops_s','p50_ms','p95_ms','p99_ms','max_ms']:
    xs=[r[metric] for r in runs];item[metric]={'median':stats.median(xs),'min':min(xs),'max':max(xs)}
   summary.append(item)
 for r in rows:
  if 'profile_file' not in r:continue
  raw=json.load(gzip.open(source/r['profile_file']));assert raw['limit']==8000 and raw['dropped']==0
  records=raw['records'];assert len(records)==8000
  for row in records:row['phases']=phases(row)
  slow=sorted(records,key=lambda r:r['request_ns'])[-80:]
  stages={name:{'mean_ms':stats.mean(x['phases'][name] for x in records)/1e6,
                'p99_ms':percentile([x['phases'][name] for x in records],.99)/1e6,
                'slowest_1pct_mean_ms':stats.mean(x['phases'][name] for x in slow)/1e6}
          for name in records[0]['phases']}
  groups={}
  for row in records:groups.setdefault(tuple(row['diagnostic']['trace'][1:]),[]).append(row)
  detail.append({'variant':r['variant'],'clients':r['clients'],'rep':r['rep'],'stages':stages,
                 'groups':len(groups),'updates_per_group':len(records)/len(groups),
                 'group_sync_mean_ms':stats.mean(g[0]['phases']['log sync'] for g in groups.values())/1e6,
                 'request_mean_ms':stats.mean(x['request_ns'] for x in records)/1e6,
                 'slowest_1pct_mean_ms':stats.mean(x['request_ns'] for x in slow)/1e6,
                 'validation_group_mean_us':stats.mean(g[0]['diagnostic']['costs'][0] for g in groups.values())/1e3,
                 'encoding_group_mean_us':stats.mean(g[0]['diagnostic']['costs'][1] for g in groups.values())/1e3})
 (out/'summary.json').write_text(json.dumps({'runs':summary,'diagnosis':detail},indent=2)+'\n')
 def find(v,c):return next(s for s in summary if s['variant']==v and s['clients']==c)
 table=['| Connections | Mode | Updates/s | p50 ms | p95 ms | p99 ms | Worst maximum ms |','|---:|---|---:|---:|---:|---:|---:|']
 for item in summary:
  table.append('| '+str(item['clients'])+' | '+item['variant']+' | '+' | '.join(f"{item[m]['median']:.3f}" for m in ['ops_s','p50_ms','p95_ms','p99_ms'])+f" | {item['max_ms']['max']:.3f} |")
 (out/'table.md').write_text('\n'.join(table)+'\n')
 import matplotlib
 matplotlib.use('Agg')
 import matplotlib.pyplot as plt
 colors=['#667085','#007f85']
 fig,axes=plt.subplots(1,2,figsize=(11,4.8),layout='constrained')
 for ax,metric,title in zip(axes,['ops_s','p99_ms'],['Successful updates / second','Client-observed p99 (ms)']):
  for j,v in enumerate(variants[:2]):
   data=[find(v,c)[metric] for c in [1,4]];values=[x['median'] for x in data];xs=[i+(j-.5)*.3 for i in range(2)]
   ax.bar(xs,values,.29,color=colors[j],label=v,yerr=[[x['median']-x['min'] for x in data],[x['max']-x['median'] for x in data]],capsize=3)
   for x,y,d in zip(xs,values,data):ax.text(x,d['max'],f'{y:.1f}' if metric=='ops_s' else f'{y:.2f}',ha='center',va='bottom',fontsize=9)
  ax.set_xticks([0,1],['1 connection','4 connections']);ax.set_title(title);ax.spines[['top','right']].set_visible(False);ax.set_ylim(0,ax.get_ylim()[1]*1.16)
 axes[0].legend();fig.suptitle('Durable service updates: omit collection wait for a sole connection')
 fig.supxlabel('Instrumentation OFF; median and range of 3 runs. 100k rows / 256B / 8,000 updates per run.\nTCP Python clients; 2 CPUs and 4 GiB per container. This is not an engine-only ceiling.',fontsize=9)
 for ext in ['svg','png']:fig.savefig(out/f'service.{ext}',dpi=140)
 plt.close(fig)
 # Means (not percentile addition) decompose each request exactly.
 fig,axes=plt.subplots(1,2,figsize=(12,5.5),layout='constrained')
 buckets={'Log sync':['log sync'],'Batch collection':['batch collection'],'Queue + locks':['leader queue','writer lock','state lock','publication lock'],
          'Build + write + publish':['validation / encoding / bookkeeping','log reservation','log write','publication'],
          'Reply + other server':['dispatch preparation','queue submission','selection overhead','cleanup / replies ahead','reply wake / leader exit','dispatch finish'],
          'Transport / client residual':['transport / client residual']}
 palette=['#1b6d85','#e0a438','#9467bd','#479e83','#778899','#c7c7c7']
 for ax,field,title in zip(axes,['mean_ms','slowest_1pct_mean_ms'],['All requests: mean breakdown','Slowest 1% requests: mean breakdown']):
  bottom=[0]*4;labels=[]
  for j,(name,names) in enumerate(buckets.items()):
   values=[];labels=[]
   for c in [1,4]:
    for v in ['before-diag','candidate-diag']:
     subset=[d for d in detail if d['clients']==c and d['variant']==v]
     values.append(stats.mean(sum(d['stages'][n][field] for n in names) for d in subset))
     labels.append(('Before' if v=='before-diag' else 'Candidate')+f'\n{c} conn.')
   ax.bar(range(4),values,bottom=bottom,color=palette[j],label=name);bottom=[a+b for a,b in zip(bottom,values)]
  ax.set_xticks(range(4),labels);ax.set_ylabel('Milliseconds');ax.set_title(title);ax.spines[['top','right']].set_visible(False)
 axes[0].legend(fontsize=8);fig.suptitle('Synchronization dominates; smaller phases are measured separately')
 fig.supxlabel('Instrumentation ON; average of 3 run means. Tail subsets selected by actual per-request duration.\nOverlapping batch durations are request-weighted elapsed time, not additive CPU work.',fontsize=9)
 for ext in ['svg','png']:fig.savefig(out/f'breakdown.{ext}',dpi=140)
 plt.close(fig)
 for svg in out.glob('*.svg'):
  svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines())+'\n')
 print('\n'.join(table))

if __name__=='__main__':main()
