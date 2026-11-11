"""Paired service comparison using the original operation implementation."""
import argparse,gzip,hashlib,itertools,json,os,pathlib,random,secrets,subprocess,time,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
def run(*args,env=None,timeout=600):
 r=subprocess.run(args,capture_output=True,text=True,encoding='utf-8',errors='replace',env=env,timeout=timeout)
 if r.returncode:raise RuntimeError(r.stderr[-4000:]+r.stdout[-1500:])
 return r.stdout.strip()
def main():
 p=argparse.ArgumentParser();p.add_argument('--variant',action='append',required=True,help='name=image');p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--reps',type=int,default=3);p.add_argument('--ops',type=int,default=8000);p.add_argument('--clients',type=int,nargs='+',default=[1,4]);p.add_argument('--workloads',nargs='+',default=['get','range100','update']);p.add_argument('--profile',action='store_true');p.add_argument('--profile-variants',nargs='*',default=[]);p.add_argument('--fixed-variants',nargs='*',default=[]);p.add_argument('--fixed-metrics-variants',nargs='*',default=[]);p.add_argument('--capacity',type=int,default=64*1024*1024);p.add_argument('--reopen-image',default='catdb-fixed-log:local');args=p.parse_args()
 out=args.out.resolve();out.mkdir(parents=True,exist_ok=False);variants=dict(x.split('=',1) for x in args.variant)
 meta={'utc':__import__('datetime').datetime.now(__import__('datetime').timezone.utc).isoformat(),'rows':100000,'write_measurement':'application WAL bytes plus separate cgroup-attributed block write bytes/requests; excludes complete device/firmware amplification','value_bytes':256,'ops':args.ops,'reps':args.reps,'profile':args.profile,'profile_variants':args.profile_variants,'fixed_variants':args.fixed_variants,'capacity':args.capacity,'fixed_metrics_variants':args.fixed_metrics_variants,'reopen_image':run('docker','image','inspect',args.reopen_image,'--format','{{.Id}}'),'clients':args.clients,'workloads':args.workloads,'server_cpus':2,'client_cpus':2,'memory_gib_each':4,'variants':{k:{'image':v,'id':run('docker','image','inspect',v,'--format','{{.Id}}')} for k,v in variants.items()},'source_sha256':{str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [ROOT/'benchmarks/runner.py',ROOT/'benchmarks/fixed-log/client.py',ROOT/'benchmarks/fixed-log/io_run.py',ROOT/'src/fixed_log.rs',ROOT/'src/transactions.rs',ROOT/'Cargo.toml',ROOT/'src/group_commit.rs',ROOT/'src/service.rs']},'runs':[]};results=[]
 def save():
  (out/'environment.json').write_text(json.dumps(meta,indent=2)+'\n',encoding='utf-8');(out/'results.json').write_text(json.dumps(results,indent=2)+'\n',encoding='utf-8')
 save()
 for rep in range(1,args.reps+1):
  order=list(variants)
  if rep%2==0:order.reverse()
  for variant in order:
   profile=args.profile or variant in args.profile_variants
   prefix='catdb-update-'+uuid.uuid4().hex[:10];net=prefix+'-net';vol=prefix+'-data';client=prefix+'-client';server=prefix+'-server';owned=[]
   env=dict(os.environ,BENCH_PASSWORD='B!'+secrets.token_hex(16),BENCH_TOKEN=secrets.token_hex(32),BENCH_ROWS='100000',BENCH_OPS=str(args.ops))
   entry={'variant':variant,'rep':rep,'status':'running','checks':[]};meta['runs'].append(entry);save();print(f'{variant} r{rep}: preparing',flush=True)
   try:
    run('docker','network','create','--internal',net);run('docker','volume','create',vol)
    common=['--network',net,'--cpus','2','--memory','4g','-v',vol+':/data']
    run('docker','run','-d','--name',client,*common,'-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','BENCH_PASSWORD','-e','BENCH_TOKEN','-e','BENCH_ROWS','-e','BENCH_OPS','catdb-comparison:local',env=env);owned.append(client)
    def execute(*a):return json.loads(run('docker','exec','-e','PYTHONPATH=/bench',client,'python','/bench-src/fixed-log/client.py',*map(str,a)))
    execute('init')
    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path('/data/token');p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400);os.chown(p,10001,10001);os.chown('/data',10001,10001)")
    diagnostic=['-e','CATDB_DIAGNOSTICS=1'] if profile else []
    if variant in args.fixed_variants:diagnostic+=['-e',f'CATDB_EXPERIMENTAL_FIXED_LOG_BYTES={args.capacity}']
    run('docker','run','-d','--name',server,*common,'--network-alias','database','--user','10001:10001',*diagnostic,variants[variant],'silverhamu-server','--db','/data/catdb.wal','--bind','0.0.0.0:7400','--allow-remote','--token-file','/data/token','--transaction-timeout-secs','3600');owned.append(server)
    for attempt in range(30):
     try:entry['checks'].append({'stage':'initial',**execute('verify')});break
     except RuntimeError:
      if run('docker','inspect',server,'--format','{{.State.Running}}')!='true':raise RuntimeError(run('docker','logs',server) or 'server exited; inspect stderr')
      if attempt==29:raise
      time.sleep(1)
    def counters():
     code="import json,pathlib,struct,zlib;p=pathlib.Path('/data/catdb.wal');f=p.open('rb');h=f.read(48);size=p.stat().st_size;end=size;v=struct.unpack_from('<I',h,8)[0];f.seek(size-8192);first=f.read(64) if v==3 else b'';f.seek(size-4096);slots=[first,f.read(64)] if v==3 else [];valid=[struct.unpack_from('<QQ',b,8) for b in slots if zlib.crc32(b[:60])==struct.unpack_from('<I',b,60)[0]];end=max(valid)[1] if valid else end;stats=[dict(pair.split('=') for pair in line.split()[1:]) for line in pathlib.Path('/sys/fs/cgroup/io.stat').read_text().splitlines()];print(json.dumps(dict(physical_bytes=size,logical_bytes=end,end_generation=max(valid)[0] if valid else 0,block_write_bytes=sum(int(s.get('wbytes',0)) for s in stats),block_write_requests=sum(int(s.get('wios',0)) for s in stats))))"
     return json.loads(run('docker','exec',server,'python','-c',code))
    def settled_counters():
     previous=counters()
     for _ in range(20):
      time.sleep(.1);current=counters()
      if current==previous:return current
      previous=current
     raise RuntimeError('I/O counters did not settle outside timing')
    entry['initial_io']=settled_counters();save()
    cases=list(itertools.product(args.workloads,args.clients));random.Random(912+rep).shuffle(cases)
    for workload,clients in cases:
     before=settled_counters()
     metrics_before=execute('metrics')['metrics'] if variant in args.fixed_metrics_variants else None
     r=execute('measure',workload,clients,rep,int(profile));r['variant']=variant
     after=settled_counters();r['io_before']=before;r['io_after']=after
     r['io_delta']={key:after[key]-before[key] for key in before}
     r['application_written_bytes']=r['io_delta']['logical_bytes']+84*r['io_delta']['end_generation']
     if metrics_before is not None:
      metrics_after=execute('metrics')['metrics'];r['fixed_sync_metrics']=[a-b for a,b in zip(metrics_after,metrics_before)]
     for field in ['latencies_ns','profile']:
      if field in r:
       name=f'{variant}-r{rep}-{workload}-c{clients}-{field}.json.gz'
       with gzip.open(out/name,'wt',encoding='utf-8') as f:json.dump(r.pop(field),f)
       r[field+'_file']=name
     results.append(r);save();print(f"{variant} r{rep} {workload} c{clients}: {r['ops_s']:.1f} ops/s p99 {r['p99_ms']:.3f} ms errors {len(r['errors'])}",flush=True)
     entry['checks'].append({'stage':f'{workload}-{clients}',**execute('verify')});save()
     if r['errors']:raise RuntimeError('Operation errors; no retries; see results')
    run('docker','stop','--time','60',server)
    entry['reopen_probe']=json.loads(run('docker','run','--rm','--user','10001:10001','--cpus','2','--memory','4g','-v',vol+':/data',args.reopen_image,'reopen-bench','/data/catdb.wal'))
    assert entry['reopen_probe']['rows']==100000
    run('docker','start',server)
    for attempt in range(30):
     try:check=execute('verify');break
     except RuntimeError:
      if attempt==29:raise
      time.sleep(1)
    entry['checks'].append({'stage':'reopen',**check});entry['status']='passed';save()
   except Exception as e:
    logs=subprocess.run(['docker','logs',server],capture_output=True,text=True,encoding='utf-8',errors='replace')
    entry['server_log']=(logs.stdout+logs.stderr).replace(env['BENCH_PASSWORD'],'[redacted]').replace(env['BENCH_TOKEN'],'[redacted]')
    entry['status']='failed';entry['error']=str(e).replace(env['BENCH_PASSWORD'],'[redacted]').replace(env['BENCH_TOKEN'],'[redacted]');save();raise
   finally:
    for name in reversed(owned):
     try:run('docker','rm','-f',name)
     except Exception:pass
    for kind,name in [('volume',vol),('network',net)]:
     try:run('docker',kind,'rm',name)
     except Exception:pass
 print('Complete '+str(out),flush=True)
if __name__=='__main__':main()
