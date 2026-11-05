import csv,gzip,json,math,statistics,sys
from pathlib import Path
sys.stdout.reconfigure(encoding="utf-8")
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
service=Path(sys.argv[1]);aux=Path(sys.argv[2]);out=Path(sys.argv[3]);out.mkdir(parents=True,exist_ok=True)
results=json.loads((service/'results.json').read_text());assert len(results)==18
assert all(r['completed']==8000 and not r['errors'] for r in results)
for p in service.iterdir():
 if p.is_file(): (out/(p.name if p.name.endswith('.gz') else 'service-'+p.name)).write_bytes(p.read_bytes())
variants=['baseline','direct','prepare'];colors=['#667085','#007f85','#be6a00']
summary=[]
for clients in [1,4]:
 for v in variants:
  rows=[r for r in results if r['clients']==clients and r['variant']==v];assert len(rows)==3
  item=dict(clients=clients,variant=v)
  for metric in ['ops_s','p50_ms','p95_ms','p99_ms','max_ms']:
   values=[r[metric] for r in rows];item[metric]=dict(median=statistics.median(values),minimum=min(values),maximum=max(values))
  summary.append(item)
fig,axes=plt.subplots(1,2,figsize=(11,4.8),layout='constrained')
for ax,metric,label in zip(axes,['ops_s','p99_ms'],['Successful updates / second','Client-observed p99 (ms)']):
 for j,(variant,color) in enumerate(zip(variants,colors)):
  data=[s[metric] for s in summary if s['variant']==variant];values=[d['median'] for d in data];x=[i+(j-1)*.24 for i in range(2)]
  ax.bar(x,values,.23,color=color,label=variant,yerr=[[v-d['minimum'] for v,d in zip(values,data)],[d['maximum']-v for v,d in zip(values,data)]],capsize=3)
  for xx,v,d in zip(x,values,data):ax.text(xx,d['maximum'],f'{v:.1f}' if metric=='ops_s' else f'{v:.2f}',ha='center',va='bottom',fontsize=8)
 ax.set_xticks([0,1],['1 connection','4 connections']);ax.set_title(label);ax.set_ylim(0,ax.get_ylim()[1]*1.15);ax.spines[['top','right']].set_visible(False)
axes[0].legend(loc='upper left');fig.suptitle('WAL preparation: 100k rows, 256-byte values, durable updates')
fig.supxlabel('Median and range of 3 runs; 8,000 operations/run. Single-request TCP API; 2 CPUs / 4 GiB per container.',fontsize=9)
fig.savefig(out/'service.svg');fig.savefig(out/'service.png',dpi=130);plt.close(fig)
def quantile(values,p):
 values=sorted(values);return values[max(0,math.ceil(len(values)*p)-1)] if values else None
aux_rows=[]
for path in sorted(aux.glob('*/samples.csv')):
 variant,shape,repeat=path.parent.name.split('-');rows=[{k:int(v) for k,v in r.items()} for r in csv.DictReader(path.open())];assert len(rows)==512
 meta=json.loads((path.parent/'metadata.json').read_text());assert meta['counted']
 accepted=[r for r in rows if r['sequence']>0];failed=[r for r in rows if r['sequence']==0]
 if shape!='hot':assert not failed
 item=dict(variant=variant,shape=shape,repeat=int(repeat),attempts=len(rows),accepted=len(accepted),rejected=len(failed),ops_s=len(accepted)/(meta['elapsed_ns']/1e9),all_p99_ms=quantile([r['ns'] for r in rows],.99)/1e6,accepted_p99_ms=quantile([r['ns'] for r in accepted],.99)/1e6,maximum_ms=max(r['ns'] for r in rows)/1e6,writer_preparation_p99_us=quantile([r['writer_preparation_ns'] for r in accepted],.99)/1e3,prepare_p99_us=quantile([r['prepare_ns'] for r in rows],.99)/1e3,prepared_bytes=sum(r['prepared_bytes'] for r in rows),discarded_bytes=sum(r['discarded_bytes'] for r in rows),commit_thread_alloc_calls=sum(r['calls'] for r in rows),commit_thread_alloc_bytes=sum(r['bytes'] for r in rows),metadata=meta)
 encode=[{k:int(v) for k,v in r.items()} for r in csv.DictReader((path.parent/'encoding.csv').open())];assert len(encode)==128
 item['encode_ns']=statistics.median(r['ns'] for r in encode);item['encode_calls']=statistics.median(r['calls'] for r in encode);item['encode_bytes']=statistics.median(r['bytes'] for r in encode)
 aux_rows.append(item)
 for name in ['samples.csv','encoding.csv','metadata.json']:
  with gzip.open(out/(path.parent.name+'-'+name+'.gz'),'wb') as f:f.write((path.parent/name).read_bytes())
assert len(aux_rows)==27
(out/'summary.json').write_text(json.dumps(dict(service=summary,auxiliary=aux_rows),indent=2)+'\n',encoding='utf-8')
lines=['| Connections | Variant | Successful ops/s | p99 ms | Worst run maximum ms |','|---:|---|---:|---:|---:|']
for s in summary:lines.append(f"| {s['clients']} | {s['variant']} | {s['ops_s']['median']:.1f} | {s['p99_ms']['median']:.3f} | {s['max_ms']['maximum']:.3f} |")
lines+=['','Auxiliary Windows engine runs: instrumentation enabled, four barrier-synchronized writers. Medians of three trials. Not directly comparable to the service chart.','','| Shape | Variant | Accepted ops/s | Accepted p99 ms | Writer preparation p99 µs | Encode allocations | Encode bytes | Discarded preparation bytes |','|---|---|---:|---:|---:|---:|---:|---:|']
def med(shape,v,k):return statistics.median(r[k] for r in aux_rows if r['shape']==shape and r['variant']==v)
for shape in ['small','large','hot']:
 for v in variants:lines.append(f"| {shape} | {v} | {med(shape,v,'ops_s'):.1f} | {med(shape,v,'accepted_p99_ms'):.3f} | {med(shape,v,'writer_preparation_p99_us'):.1f} | {med(shape,v,'encode_calls'):.0f} | {med(shape,v,'encode_bytes'):.0f} | {med(shape,v,'discarded_bytes'):.0f} |")
(out/'table.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
fig,axes=plt.subplots(2,2,figsize=(12,8),layout='constrained')
for ax,(metric,label,scale) in zip(axes.flat,[('accepted_p99_ms','Accepted commit p99 (ms)',1),('writer_preparation_p99_us','Writer-owned preparation p99 (µs)',1),('encode_bytes','One frame: cumulative allocation (KiB)',1024),('discarded_bytes','Discarded prepared frames (KiB / 512 attempts)',1024)]):
 for j,(v,color) in enumerate(zip(variants,colors)):
  values=[med(shape,v,metric)/scale for shape in ['small','large','hot']];x=[i+(j-1)*.25 for i in range(3)]
  ax.bar(x,values,.24,label=v,color=color)
  for xx,value in zip(x,values):ax.text(xx,value,f'{value:.1f}',ha='center',va='bottom',fontsize=8)
 ax.set_xticks(range(3),['256B / disjoint','64KiB / disjoint','256B / hot key']);ax.set_title(label);ax.set_ylim(0,ax.get_ylim()[1]*1.15);ax.spines[['top','right']].set_visible(False)
axes[0,0].legend(loc='upper left');fig.suptitle('Auxiliary engine experiment: bounded speculative WAL preparation')
fig.supxlabel('3 trials; Windows; 100k rows; 4 synchronized writers; 512 attempts/trial. Allocation + lock timing enabled.\nWriter-owned interval includes state-lock acquisition; hot-key conflicts are not retried.',fontsize=9)
fig.savefig(out/'auxiliary.svg');fig.savefig(out/'auxiliary.png',dpi=130);plt.close(fig)
print('\n'.join(lines))
