from pathlib import Path
import csv,json,math,argparse
p=argparse.ArgumentParser();p.add_argument('--input',type=Path,nargs='+',required=True);a=p.parse_args();root=a.input[0];out=root/'report';out.mkdir(exist_ok=True)
summaries=[];raw=[]
def q(v,p):return sorted(v)[max(0,math.ceil(len(v)*p)-1)]/1e6
for path in sorted(path for folder in a.input for path in folder.glob('*.log')):
 if not path.name.startswith(('pilot-','main-')):continue
 lines=path.read_text(encoding='utf-8-sig').splitlines()
 if not any('test result: ok.' in l for l in lines):continue
 rs=[];head=None;reopens=[]
 for l in lines:
  i=l.find('PHASE_')
  if i<0:continue
  f=l[i:].split(',')
  if f[0]=='PHASE_SUMMARY':head=f
  elif f[0]=='PHASE_REOPEN':reopens.append(int(f[4]))
  elif f[0]=='PHASE_ROW':
   keys=['index','start_ns','total_ns','append_ns','switch_ns','append_prepare_ns','append_write_ns','append_sync_ns','append_publish_ns','switch_prepare_ns','switch_write_ns','switch_sync_ns','switch_publish_ns']
   r=dict(zip(keys,map(int,f[4:]),strict=True));r.update(stage=path.name.split('-')[0],mode=f[1],trial=int(f[2]),trace=f[3]=='true');rs.append(r)
 assert head is not None and len(rs)==int(head[4]) and head[8]=='true' and len(reopens)==100
 assert [r['index'] for r in rs]==list(range(len(rs)))
 for r in rs:
  assert r['total_ns']>=r['append_ns']+r['switch_ns']
  if r['trace']:
   for op in ['append','switch']:assert sum(r[f'{op}_{x}_ns'] for x in ['prepare','write','sync','publish'])==r[op+'_ns']
  else:assert sum(r[k] for k in keys[5:])==0
 for l,r in zip(rs,rs[1:]):assert r['start_ns']>=l['start_ns']+l['total_ns']
 s={k:rs[0][k] for k in ['stage','mode','trial','trace']};s.update(count=len(rs),ops_per_s=len(rs)*1e9/int(head[5]),submitted_bytes=int(head[6]),sync_calls=int(head[7]),over10=sum(r['total_ns']>10e6 for r in rs),over20=sum(r['total_ns']>20e6 for r in rs))
 for op in ['total','append','switch']:
  vals=[r[op+'_ns'] for r in rs if op!='switch' or r['switch_ns']]
  for label,pct in [('p50',.5),('p99',.99),('max',1)]:s[f'{op}_{label}_ms']=q(vals,pct)
 s['reopen_p99_ms']=q(reopens,.99);worst=max(rs,key=lambda r:r['total_ns']);s['worst_index']=worst['index'];s['worst_sync_ms']=(worst['append_sync_ns']+worst['switch_sync_ns'])/1e6;s['sync_fraction']=sum(r['append_sync_ns']+r['switch_sync_ns'] for r in rs)/sum(r['total_ns'] for r in rs)
 s['worst_prepare_ms']=(worst['append_prepare_ns']+worst['switch_prepare_ns'])/1e6;s['worst_write_ms']=(worst['append_write_ns']+worst['switch_write_ns'])/1e6;s['worst_publish_ms']=(worst['append_publish_ns']+worst['switch_publish_ns'])/1e6;s['worst_residual_ms']=(worst['total_ns']-worst['append_ns']-worst['switch_ns'])/1e6
 summaries.append(s);raw.extend(rs)
keys=[(s['stage'],s['mode'],s['trial'],s['trace']) for s in summaries]
assert len(keys)==len(set(keys)), 'Duplicate trials'
for stage,count in [('pilot',8),('main',6)]:
 found=[s for s in summaries if s['stage']==stage]
 if found:assert len(found)==count, f'Incomplete {stage} run set'
for filename,rows in [('trials.csv',summaries),('requests.csv',raw)]:
 with (out/filename).open('w',newline='') as f:w=csv.DictWriter(f,fieldnames=list(rows[0]));w.writeheader();w.writerows(rows)
(out/'summary.json').write_text(json.dumps(summaries,indent=2)+'\n')
for s in summaries:print(json.dumps(s))
