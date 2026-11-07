from pathlib import Path
import argparse,csv,json,math
p=argparse.ArgumentParser();p.add_argument('--input',type=Path,required=True);a=p.parse_args();root=a.input;out=root/'report';out.mkdir(exist_ok=True)
def q(vals,p):return sorted(vals)[max(0,math.ceil(len(vals)*p)-1)]/1e6
allrows=[];summaries=[]
order=[(4,1),(16,1),(64,1),(16,2),(64,2),(4,2),(64,3),(4,3),(16,3)]
for cap,trial in order:
 path=root/f'cap-{cap}-{trial}.log';lines=path.read_text(encoding='utf-8-sig').splitlines();assert any('test result: ok. 1 passed;' in l for l in lines);head=None;updates=[];reopens=[]
 for l in lines:
  i=l.find('CAP_')
  if i<0:continue
  f=l[i:].split(',')
  if f[0]=='CAP_SUMMARY':head=f
  elif f[0]=='CAP_UPDATE':updates.append(tuple(map(int,f[3:])))
  elif f[0]=='CAP_REOPEN':reopens.append(tuple(map(int,f[3:])))
 assert head is not None and head[-1]=='true' and len(updates)==2048 and len(reopens)==100
 assert [u[0] for u in updates]==list(range(2048));assert [r[0] for r in reopens]==list(range(100));assert all(t>=ap+sw for _,t,ap,sw in updates)
 switches=[u[3] for u in updates if u[3]];assert len(switches)==2048//cap
 assert int(head[5])==2048+2*2048//cap;assert int(head[6])==(4+4*(cap+1))*16384
 s={'capacity':cap,'trial':trial,'ops_per_s':2048e9/int(head[3]),'submitted_bytes':int(head[4]),'sync_calls':int(head[5]),'file_bytes':int(head[6]),'initialization_ms':int(head[7])/1e6,'switches':len(switches),'over10':sum(u[1]>10e6 for u in updates),'over20':sum(u[1]>20e6 for u in updates)}
 for name,vals in [('request',[u[1] for u in updates]),('append',[u[2] for u in updates]),('switch',switches),('reopen',[r[1] for r in reopens])]:
  for label,pct in [('p50',.5),('p99',.99),('max',1)]:s[f'{name}_{label}_ms']=q(vals,pct)
 summaries.append(s)
 for i,t,ap,sw in updates:allrows.append([cap,trial,'update',i,t,ap,sw])
 for i,ns in reopens:allrows.append([cap,trial,'reopen',i,ns,'',''])
with (out/'trials.csv').open('w',newline='') as f:w=csv.DictWriter(f,fieldnames=list(summaries[0]));w.writeheader();w.writerows(summaries)
with (out/'requests.csv').open('w',newline='') as f:w=csv.writer(f);w.writerow(['capacity','trial','operation','index','elapsed_ns','append_ns','switch_ns']);w.writerows(allrows)
(out/'summary.json').write_text(json.dumps(summaries,indent=2)+'\n')
for s in summaries:print(json.dumps(s))
