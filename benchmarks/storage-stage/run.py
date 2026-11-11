"""Paired service comparison using the original operation implementation."""
import argparse,gzip,hashlib,itertools,json,os,pathlib,random,secrets,subprocess,time,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
def run(*args,env=None,timeout=600):
 r=subprocess.run(args,capture_output=True,text=True,encoding='utf-8',errors='replace',env=env,timeout=timeout)
 if r.returncode:raise RuntimeError(r.stderr[-4000:]+r.stdout[-1500:])
 return r.stdout.strip()
def main():
 p=argparse.ArgumentParser();p.add_argument('--variant',action='append',required=True,help='name=image');p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--reps',type=int,default=3);p.add_argument('--ops',type=int,default=8000);p.add_argument('--cases',nargs='+',default=['1:200','1:250','4:400','4:800']);p.add_argument('--scenarios',nargs='+',default=['normal']);args=p.parse_args()
 out=args.out.resolve();out.mkdir(parents=True,exist_ok=False);variants=dict(x.split('=',1) for x in args.variant)
 meta={'utc':__import__('datetime').datetime.now(__import__('datetime').timezone.utc).isoformat(),'rows':100000,'value_bytes':256,'ops':args.ops,'reps':args.reps,'cases':args.cases,'scenarios':args.scenarios,'clients':sorted({int(c.split(':')[0]) for c in args.cases}),'workloads':['update'],'server_cpus':2,'client_cpus':2,'memory_gib_each':4,'variants':{k:{'image':v,'id':run('docker','image','inspect',v,'--format','{{.Id}}')} for k,v in variants.items()},'source_sha256':{str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [ROOT/'benchmarks/storage-stage/run.py',ROOT/'benchmarks/runner.py',ROOT/'benchmarks/storage-stage/client.py',ROOT/'benchmarks/storage-stage/schedule.py',ROOT/'src/group_commit.rs',ROOT/'src/service.rs',ROOT/'benchmarks/storage-stage/collect.py']},'runs':[]};results=[]
 def save():
  (out/'environment.json').write_text(json.dumps(meta,indent=2)+'\n',encoding='utf-8');(out/'results.json').write_text(json.dumps(results,indent=2)+'\n',encoding='utf-8')
 save()
 for rep in range(1,args.reps+1):
  order=list(variants)
  if rep%2==0:order.reverse()
  for variant in order:
   prefix='catdb-request-'+uuid.uuid4().hex[:10];net=prefix+'-net';vol=prefix+'-data';client=prefix+'-client';server=prefix+'-server';owned=[]
   env=dict(os.environ,BENCH_PASSWORD='B!'+secrets.token_hex(16),BENCH_TOKEN=secrets.token_hex(32),BENCH_ROWS='100000',BENCH_OPS=str(args.ops),BENCH_DIAGNOSTIC='1' if 'profile' in variant else '0')
   entry={'variant':variant,'rep':rep,'status':'running','checks':[]};meta['runs'].append(entry);save();print(f'{variant} r{rep}: preparing',flush=True)
   try:
    run('docker','network','create','--internal',net);run('docker','volume','create',vol)
    common=['--network',net,'--cpus','2','--memory','4g','-v',vol+':/data']
    run('docker','run','-d','--name',client,*common,'-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','BENCH_PASSWORD','-e','BENCH_TOKEN','-e','BENCH_ROWS','-e','BENCH_OPS','-e','BENCH_DIAGNOSTIC','catdb-comparison:local',env=env);owned.append(client)
    def execute(*a):return json.loads(run('docker','exec','-e','PYTHONPATH=/bench',client,'python','/bench-src/storage-stage/client.py',*map(str,a)))
    execute('init')
    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path('/data/token');p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400);os.chown(p,10001,10001);os.chown('/data',10001,10001)")
    diagnostic=['-e','CATDB_BENCH_STALL=1']
    if 'profile' in variant:diagnostic+=['-e','CATDB_DIAGNOSTICS=buffer']
    run('docker','run','-d','--name',server,*common,'--network-alias','database','--user','10001:10001',*diagnostic,variants[variant],'silverhamu-server','--db','/data/catdb.wal','--bind','0.0.0.0:7400','--allow-remote','--token-file','/data/token','--transaction-timeout-secs','3600');owned.append(server)
    for attempt in range(30):
     try:entry['checks'].append({'stage':'initial',**execute('verify')});break
     except RuntimeError:
      if attempt==29:raise
      time.sleep(1)
    cases=[(*map(int,c.split(':')),scenario) for c in args.cases for scenario in args.scenarios];random.Random(912+rep).shuffle(cases)
    for clients,rate,scenario in cases:
     workload='update'
     trace_name=f'{variant}-r{rep}-c{clients}-rate{rate}-{scenario}'
     collector=None
     if 'trace' in variant:
      trace_dir=out/trace_name;trace_dir.mkdir()
      collector=prefix+'-collector-'+uuid.uuid4().hex[:6]
      run('docker','run','-d','--name',collector,'--network','none','--pid','host','--read-only','--cap-add','SYS_ADMIN','--tmpfs','/trace:rw,nosuid,nodev,mode=700','-v',str(trace_dir)+':/control','-v',str(ROOT/'benchmarks/storage-stage')+':/collector:ro','catdb-comparison:local','python','/collector/collect.py');owned.append(collector)
      deadline=time.monotonic()+30
      while not (trace_dir/'ready').exists():
       if time.monotonic()>deadline:
        log=subprocess.run(['docker','logs',collector],capture_output=True,text=True)
        (trace_dir/'collector-error.txt').write_text(log.stdout+log.stderr)
        raise RuntimeError('collector not ready: '+log.stdout+log.stderr)
       time.sleep(.05)
     try:r=execute('measure',clients,rep,rate,scenario,8)
     finally:
      if collector:
       (trace_dir/'stop').write_text('stop')
       status=run('docker','wait',collector,timeout=30)
       if status!='0':raise RuntimeError('collector failed: '+run('docker','logs',collector))
       assert json.loads((trace_dir/'metadata.json').read_text()).get('request_probe_count')==15, 'wrong collector'
     r['variant']=variant
     if collector:r['kernel_trace']=trace_name
     for field in ['events','batches','diagnostics']:
      if field in r:
       name=f'{variant}-r{rep}-{workload}-c{clients}-rate{rate}-{scenario}-{field}.json.gz'
       with gzip.open(out/name,'wt',encoding='utf-8') as f:json.dump(r.pop(field),f)
       r[field+'_file']=name
     results.append(r);save();print(f"{variant} r{rep} {workload} c{clients} rate{rate} {scenario}: {r['ops_s']:.1f} ops/s response p99 {r['response']['p99_ms']:.3f} ms rejected {r['rejected']} errors {len(r['errors'])}",flush=True)
     entry['checks'].append({'stage':f'{workload}-{clients}-{rate}-{scenario}',**execute('verify')});save()
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
