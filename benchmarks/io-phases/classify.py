import collections,gzip,json,pathlib,statistics,sys
root=pathlib.Path(sys.argv[1]);out=[]
for p in sorted(root.glob('*trace.json.gz')):
 d=json.load(gzip.open(p,'rt'));sizes={v['path']:v['size'] for v in d['fds'].values()};g=[{'sync':0,'write':0,'growth':False,'db':False,'sync_count':0} for _ in d['bounds']];calls=collections.Counter()
 for e in sorted(d['events'],key=lambda e:e[7]):
  r,k,fd,off,n,result,err,start,end=e;path=d['fds'][str(fd)]['path'];a=g[r];calls[str(k)]+=1
  if k in (1,2):a['sync']+=end-start;a['sync_count']+=1
  if k in (3,4):
   a['write']+=end-start
   if off+result>sizes[path]:a['growth']=True;sizes[path]=off+result
   if path=='sqlite.db':a['db']=True
  if k==5:sizes[path]=off
 classes={}
 for name,pred in [('growing',lambda a:a['growth'] and not a['db']),('reuse',lambda a:not a['growth'] and not a['db']),('db_write',lambda a:a['db'])]:
  ids=[i for i,a in enumerate(g) if pred(a)]
  if ids:classes[name]={'requests':len(ids),'mean_sync_ms':statistics.mean(g[i]['sync'] for i in ids)/1e6,'mean_request_ms':statistics.mean(d['samples_ns'][i] for i in ids)/1e6,'sync_calls':sum(g[i]['sync_count'] for i in ids)}
 row={'file':p.name,'calls':dict(calls),'classes':classes};out.append(row);print(json.dumps(row))
if len(sys.argv)>2:pathlib.Path(sys.argv[2]).write_text(json.dumps(out,indent=2))
