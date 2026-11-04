"""Paired service comparison using the original operation implementation."""
import argparse,gzip,hashlib,itertools,json,os,pathlib,random,secrets,subprocess,time,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
def run(*args,env=None,timeout=600):
 r=subprocess.run(args,capture_output=True,text=True,encoding='utf-8',errors='replace',env=env,timeout=timeout)
 if r.returncode:raise RuntimeError(r.stderr[-4000:]+r.stdout[-1500:])
 return r.stdout.strip()
def main():
 p=argparse.ArgumentParser();p.add_argument('--variant',action='append',required=True,help='name=image');p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--reps',type=int,default=3);p.add_argument('--ops',type=int,default=8000);p.add_argument('--clients',type=int,nargs='+',default=[1,4]);p.add_argument('--workloads',nargs='+',default=['get','range100','update']);p.add_argument('--single-variants',nargs='*',default=[]);p.add_argument('--profile',action='store_true');p.add_argument('--profile-variants',nargs='*',default=[]);args=p.parse_args()
 out=args.out.resolve();out.mkdir(parents=True,exist_ok=False);variants=dict(x.split('=',1) for x in args.variant)
 meta={'utc':__import__('datetime').datetime.now(__import__('datetime').timezone.utc).isoformat(),'rows':100000,'value_bytes':256,'ops':args.ops,'reps':args.reps,'single_variants':args.single_variants,'profile':args.profile,'profile_variants':args.profile_variants,'clients':args.clients,'workloads':args.workloads,'server_cpus':2,'client_cpus':2,'memory_gib_each':4,'variants':{k:{'image':v,'id':run('docker','image','inspect',v,'--format','{{.Id}}')} for k,v in variants.items()},'source_sha256':{str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [ROOT/'benchmarks/runner.py',ROOT/'benchmarks/single-request/client.py',ROOT/'src/group_commit.rs',ROOT/'src/service.rs']},'runs':[]};results=[]
 def save():
  (out/'environment.json').write_text(json.dumps(meta,indent=2)+'\n',encoding='utf-8');(out/'results.json').write_text(json.dumps(results,indent=2)+'\n',encoding='utf-8')
 save()
 for rep in range(1,args.reps+1):
  order=list(variants)
  if rep%2==0:order.reverse()
  for variant in order:
   profile=args.profile or variant in args.profile_variants
   prefix='catdb-single-'+uuid.uuid4().hex[:10];net=prefix+'-net';vol=prefix+'-data';client=prefix+'-client';server=prefix+'-server';owned=[]
   env=dict(os.environ,BENCH_PASSWORD='B!'+secrets.token_hex(16),BENCH_TOKEN=secrets.token_hex(32),BENCH_ROWS='100000',BENCH_OPS=str(args.ops))
   entry={'variant':variant,'rep':rep,'status':'running','checks':[]};meta['runs'].append(entry);save();print(f'{variant} r{rep}: preparing',flush=True)
   try:
    run('docker','network','create','--internal',net);run('docker','volume','create',vol)
    common=['--network',net,'--cpus','2','--memory','4g','-v',vol+':/data']
    run('docker','run','-d','--name',client,*common,'-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','BENCH_PASSWORD','-e','BENCH_TOKEN','-e','BENCH_ROWS','-e','BENCH_OPS','catdb-comparison:local',env=env);owned.append(client)
    def execute(*a):return json.loads(run('docker','exec','-e','PYTHONPATH=/bench',client,'python','/bench-src/single-request/client.py',str(int(variant in args.single_variants)),*map(str,a)))
    execute('init')
    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path('/data/token');p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400);os.chown(p,10001,10001);os.chown('/data',10001,10001)")
    diagnostic=['-e','CATDB_DIAGNOSTICS=1'] if profile else []
    run('docker','run','-d','--name',server,*common,'--network-alias','database','--user','10001:10001',*diagnostic,variants[variant],'catdb-server','--db','/data/catdb.wal','--bind','0.0.0.0:7400','--allow-remote','--token-file','/data/token','--transaction-timeout-secs','3600');owned.append(server)
    for attempt in range(30):
     try:entry['checks'].append({'stage':'initial',**execute('verify')});break
     except RuntimeError:
      if attempt==29:raise
      time.sleep(1)
    cases=list(itertools.product(args.workloads,args.clients));random.Random(912+rep).shuffle(cases)
    for workload,clients in cases:
     r=execute('measure',workload,clients,rep,int(profile));r['variant']=variant
     for field in ['latencies_ns','profile']:
      if field in r:
       name=f'{variant}-r{rep}-{workload}-c{clients}-{field}.json.gz'
       with gzip.open(out/name,'wt',encoding='utf-8') as f:json.dump(r.pop(field),f)
       r[field+'_file']=name
     results.append(r);save();print(f"{variant} r{rep} {workload} c{clients}: {r['ops_s']:.1f} ops/s p99 {r['p99_ms']:.3f} ms errors {len(r['errors'])}",flush=True)
     entry['checks'].append({'stage':f'{workload}-{clients}',**execute('verify')});save()
     if r['errors']:raise RuntimeError('Operation errors; no retries; see results')
    run('docker','restart','--time','60',server)
    for attempt in range(30):
     try:check=execute('verify');break
     except RuntimeError:
      if attempt==29:raise
      time.sleep(1)
    entry['checks'].append({'stage':'reopen',**check});entry['status']='passed';save()
   except Exception as e:
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
