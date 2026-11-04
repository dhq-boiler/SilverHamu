import collections,gzip,json,math,pathlib,statistics,sys
root=pathlib.Path(sys.argv[1]);reports=[]
for row in json.loads((root/'results.json').read_text()):
 if 'profile_file' not in row:continue
 with gzip.open(root/row['profile_file'],'rt') as f:events=json.load(f)
 stages=collections.defaultdict(list);groups=collections.Counter()
 for e in events:
  if e['op']!='commit' or not e['server']:continue
  d=e['server'];t=d['trace'];assert len(t)==8 and all(t)
  assert all(b>=a for a,b in zip(t,t[1:])),t
  finished=d['finished_ns'];started=finished-d['dispatch_ns']
  stages['before_submit'].append(t[0]-started)
  names=['batch_queue','batch_to_writer','writer_wait','validate_encode','write','sync','publish']
  for n,a,b in zip(names,t,t[1:]):stages[n].append(b-a)
  stages['release_notify'].append(finished-t[7]);stages['client_extra'].append(e['client_ns']-d['dispatch_ns'])
  groups[t[6]]+=1
 if not groups:
  assert not row["variant"].endswith("-diag"), "diagnostic response missing"
  continue
 assert sum(groups.values())==row["completed"] and max(groups.values())<=32
 summary={}
 for n,values in stages.items():
  assert min(values)>=0,(n,min(values))
  values=sorted(values);summary[n]={'mean_ms':statistics.mean(values)/1e6,'p50_ms':statistics.median(values)/1e6,'p99_ms':values[math.ceil(len(values)*.99)-1]/1e6,'max_ms':max(values)/1e6}
 reports.append({'variant':row['variant'],'rep':row['rep'],'clients':row['clients'],'workload':row['workload'],'ops_s':row['ops_s'],'p99_ms':row['p99_ms'],'commits':sum(groups.values()),'sync_groups':len(groups),'commits_per_sync':sum(groups.values())/len(groups),'batch_histogram':dict(collections.Counter(groups.values())),'stages':summary})
(root/'diagnosis.json').write_text(json.dumps(reports,indent=2)+'\n',encoding='utf-8')
print(json.dumps(reports,indent=2))
