from pathlib import Path
import csv,math,json,statistics
import argparse
parser=argparse.ArgumentParser();parser.add_argument('--input',type=Path,required=True);args=parser.parse_args()
root=args.input;out=root/'report';out.mkdir(exist_ok=True)
rows=[];summaries=[];allraw=[]
def q(vals,p):
 s=sorted(vals);return s[max(0,math.ceil(len(s)*p)-1)]/1e6
for mode,trial in [('clear',1),('reuse',1),('reuse',2),('clear',2),('clear',3),('reuse',3)]:
 path=root/f'{mode}-{trial}.log'
 if not path.exists():continue
 lines=path.read_text(encoding='utf-8-sig').splitlines()
 if not any('test result: ok.' in l for l in lines):continue
 updates=[];reopens=[];summary=None
 for line in lines:
  pos=line.find('SPEED_')
  if pos<0:continue
  fields=line[pos:].split(',');tag=fields[0]
  if tag=='SPEED_UPDATE':updates.append(tuple(map(int,fields[3:])))
  elif tag=='SPEED_REOPEN':reopens.append(tuple(map(int,fields[3:])))
  elif tag=='SPEED_SUMMARY':summary=fields
 assert summary is not None and len(updates)==2000 and len(reopens)==100
 assert [x[0] for x in updates]==list(range(2000))
 assert all(total>=append+switch for _,total,append,switch in updates)
 switch=[x[3] for x in updates if x[3]];assert len(switch)==500
 elapsed=int(summary[3]);r={'mode':mode,'trial':trial,'ops_per_s':2000e9/elapsed,'elapsed_ns':elapsed,'submitted_bytes':int(summary[4]),'sync_calls':int(summary[5])}
 for name,values in [('request',[x[1] for x in updates]),('append',[x[2] for x in updates]),('switch',switch),('reopen',[x[1] for x in reopens])]:
  for label,p in [('p50',.5),('p99',.99),('max',1)]:r[name+'_'+label+'_ms']=q(values,p)
 summaries.append(r)
 for idx,total,append,sw in updates:allraw.append([mode,trial,'update',idx,total,append,sw])
 for idx,ns in reopens:allraw.append([mode,trial,'reopen',idx,ns,'',''])
assert len(summaries)==6, 'Expected all six completed trials'
with (out/'trials.csv').open('w',newline='') as f:
 w=csv.DictWriter(f,fieldnames=list(summaries[0]));w.writeheader();w.writerows(summaries)
with (out/'requests.csv').open('w',newline='') as f:
 w=csv.writer(f);w.writerow(['mode','trial','operation','index','elapsed_ns','append_ns','switch_ns']);w.writerows(allraw)
(out/'summary.json').write_text(json.dumps(summaries,indent=2)+'\n')
for r in summaries:print(json.dumps(r))
