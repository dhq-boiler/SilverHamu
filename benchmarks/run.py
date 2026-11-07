"""Run only disposable, uniquely named Docker fixtures. Requires Docker Desktop/Linux."""
import argparse, datetime, gzip, hashlib, itertools, json, os, pathlib, random, secrets, subprocess, sys, time, uuid
ROOT=pathlib.Path(__file__).resolve().parents[1]
IMAGES={'postgres':'postgres:18','mysql':'mysql:8.4','oracle':'container-registry.oracle.com/database/free:latest-lite','catdb-service':'catdb-comparison:local'}

def run(*args,env=None,timeout=600):
    r=subprocess.run(list(args),capture_output=True,text=True,encoding='utf-8',errors='replace',env=env,timeout=timeout)
    if r.returncode:raise RuntimeError(r.stderr[-4000:]+r.stdout[-1000:])
    return r.stdout.strip()

def main():
    p=argparse.ArgumentParser();p.add_argument("--four-updates",action="store_true",help="benchmark only four-client durable updates");p.add_argument('--single-request',action='store_true');p.add_argument('--image',default='catdb-comparison:local');p.add_argument('--out',required=True,type=pathlib.Path);p.add_argument('--rows',type=int,default=100000);p.add_argument('--ops',type=int,default=8000);p.add_argument('--reps',type=int,default=3);p.add_argument('--engines',nargs='+',default=['catdb-embedded','sqlite','catdb-service','postgres','mysql','oracle']);args=p.parse_args()
    IMAGES['catdb-service']=args.image
    out=args.out.resolve();out.mkdir(parents=True,exist_ok=False)
    environment={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'rows':args.rows,'value_bytes':256,'ops_per_case':args.ops,'reps':args.reps,'single_request':args.single_request,'git_commit':run('git','-C',str(ROOT),'rev-parse','HEAD'),'docker':json.loads(run('docker','info','--format','{{json .}}')),'images':{},'source_sha256':{},'runs':[], 'skipped':{'sql-server':'SQL Server 2022 Developer/Express/Evaluation license section 5 requires prior written permission for disclosure; permission not obtained, not measured.'}}
    # Publish only hardware/runtime fields; Docker info can contain workstation names and paths.
    environment['docker']={k:environment['docker'].get(k) for k in ['ServerVersion','NCPU','MemTotal','Driver','OSType','Architecture','KernelVersion']}
    for f in ['runner.py','run.py','Dockerfile']:
        environment['source_sha256'][f]=hashlib.sha256((ROOT/'benchmarks'/f).read_bytes()).hexdigest()
    for engine in args.engines:
        image=IMAGES.get(engine,args.image)
        environment['images'][engine]=json.loads(run('docker','image','inspect',image,'--format','{{json .RepoDigests}}')) or json.loads(run('docker','image','inspect',image,'--format','{{json .Id}}'))
    all_results=[]
    def save():
        (out/'environment.json').write_text(json.dumps(environment,indent=2)+'\n',encoding='utf-8')
        (out/'results.json').write_text(json.dumps(all_results,indent=2)+'\n',encoding='utf-8')
    save()
    for rep in range(1,args.reps+1):
        order=args.engines[:] if rep%2 else list(reversed(args.engines))
        if rep==3:order=order[2:]+order[:2]
        for engine in order:
            prefix='catdb-bench-'+uuid.uuid4().hex[:10];network=prefix+'-net';volume=prefix+'-data';db=prefix+'-db';client=prefix+'-client';containers=[]
            env=dict(os.environ);env.update(BENCH_PASSWORD='B!'+secrets.token_hex(18),BENCH_TOKEN=secrets.token_hex(32),BENCH_ROWS=str(args.rows),BENCH_OPS=str(args.ops),BENCH_SINGLE_REQUEST='1' if args.single_request else '0')
            entry={'engine':engine,'rep':rep,'status':'running','verification':[]};environment['runs'].append(entry);save()
            print(f'{engine} rep {rep}: preparing',flush=True)
            try:
                run('docker','network','create','--internal',network);run('docker','volume','create',volume)
                common=['--cpus','2','--memory','4g','--network',network]
                run('docker','run','-d','--name',client,*common,'-v',volume+':/data','-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','BENCH_PASSWORD','-e','BENCH_TOKEN','-e','BENCH_ROWS','-e','BENCH_OPS','-e','BENCH_SINGLE_REQUEST',args.image,env=env);containers.append(client)
                def execute(action,*extra):
                    return json.loads(run('docker','exec','-e','PYTHONPATH=/bench',client,'python','/bench-src/runner.py',action,engine,*map(str,extra),timeout=600))
                if engine=='catdb-service':
                    execute('init')
                    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path('/data/token');p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400);os.chown(p,10001,10001)")
                    run('docker','run','-d','--name',db,*common,'--network-alias','database','--user','10001:10001','-v',volume+':/data',args.image,'catdb-server','--db','/data/catdb.wal','--bind','0.0.0.0:7400','--allow-remote','--token-file','/data/token','--transaction-timeout-secs','3600',env=env);containers.append(db)
                elif engine in ['postgres','mysql','oracle']:
                    env.update(POSTGRES_PASSWORD=env['BENCH_PASSWORD'],MYSQL_PASSWORD=env['BENCH_PASSWORD'],MYSQL_ROOT_PASSWORD=env['BENCH_PASSWORD'],ORACLE_PWD=env['BENCH_PASSWORD'])
                    opts={'postgres':['-e','POSTGRES_USER=bench','-e','POSTGRES_DB=bench','-e','POSTGRES_PASSWORD','-v',volume+':/var/lib/postgresql'],
                    'mysql':['-e','MYSQL_DATABASE=bench','-e','MYSQL_USER=bench','-e','MYSQL_PASSWORD','-e','MYSQL_ROOT_PASSWORD','-v',volume+':/var/lib/mysql'],
                    'oracle':['-e','ORACLE_PWD','--shm-size','1g','-v',volume+':/opt/oracle/oradata']}[engine]
                    tail={'postgres':['-c','fsync=on','-c','synchronous_commit=on'],'mysql':['--innodb-flush-log-at-trx-commit=1','--sync-binlog=1'],'oracle':[]}[engine]
                    run('docker','run','-d','--name',db,*common,'--network-alias','database',*opts,IMAGES[engine],*tail,env=env);containers.append(db)
                    if engine=='oracle':
                        code='import os,oracledb;d=oracledb.connect(user=\'system\',password=os.environ[\'BENCH_PASSWORD\'],dsn=\'database:1521/FREEPDB1\');c=d.cursor();c.execute("SELECT file_name FROM dba_data_files WHERE tablespace_name=\'SYSTEM\'");data=c.fetchone()[0].rsplit(\'/\',1)[0]+\'/bench-users.dbf\';c.execute("CREATE TABLESPACE users DATAFILE \'"+data+"\' SIZE 64M AUTOEXTEND ON NEXT 32M MAXSIZE 256M");p=os.environ[\'BENCH_PASSWORD\'];c.execute(\'CREATE USER bench IDENTIFIED BY "\'+p+\'" DEFAULT TABLESPACE users QUOTA 200M ON users\');c.execute(\'GRANT CREATE SESSION, CREATE TABLE TO bench\');d.close()'
                        for attempt in range(120):
                            try:run('docker','exec',client,'python','-c',"import os,oracledb;d=oracledb.connect(user='system',password=os.environ['BENCH_PASSWORD'],dsn='database:1521/FREEPDB1');d.close()",timeout=15);break
                            except RuntimeError:
                                if attempt==119:raise
                                time.sleep(2)
                        run('docker','exec',client,'python','-c',code,timeout=60)
                if engine in IMAGES:
                    for attempt in range(90):
                        try:
                            if engine=='catdb-service':
                                run('docker','exec','-e','PYTHONPATH=/bench',client,'python','-c',"import os;from catdb_client import Client;c=Client('database',7400,os.environ['BENCH_TOKEN']);assert c.request(op='ping')['ok'];c.close()")
                            else:execute('metadata')
                            break
                        except RuntimeError:
                            if attempt==89:raise
                            time.sleep(2)
                if engine!='catdb-service':execute('init')
                entry['python_packages']=run('docker','exec',client,'pip','freeze').splitlines();entry['metadata']=execute('metadata');entry['verification'].append({'stage':'initial',**execute('verify')});save()
                cases=[('update',4)] if args.four_updates else list(itertools.product(['get','range100','update'],[1,4]));random.Random(912+rep).shuffle(cases)
                for work,clients in cases:
                    result=execute('measure','--workload',work,'--clients',clients,'--rep',rep)
                    samples=result.pop('latencies_ns');name=f'{engine}-r{rep}-{work}-c{clients}.json.gz'
                    with gzip.open(out/name,'wt',encoding='utf-8') as f:json.dump(samples,f)
                    result['latency_file']=name;all_results.append(result);save()
                    print(f"{engine} r{rep} {work} c{clients}: {result['ops_s']:.1f} ops/s, p99={result['p99_ms']} ms, errors={len(result['errors'])}",flush=True)
                    entry['verification'].append({'stage':f'{work}-{clients}',**execute('verify')});save()
                    if result['errors']:raise RuntimeError('Workload errors; see raw results. No automatic retries.')
                if engine in IMAGES:
                    run('docker','restart','--time','60',db,timeout=180)
                    for attempt in range(90):
                        try:check=execute('verify');break
                        except RuntimeError:
                            if attempt==89:raise
                            time.sleep(2)
                else:check=execute('verify')
                entry['verification'].append({'stage':'reopen',**check});entry['status']='passed';save()
            except Exception as e:
                entry['status']='failed';entry['error']=str(e).replace(env['BENCH_PASSWORD'],'[redacted]').replace(env['BENCH_TOKEN'],'[redacted]');save();raise
            finally:
                for container in reversed(containers):
                    try:run('docker','rm','-f',container)
                    except Exception:pass
                for kind,name in [('volume',volume),('network',network)]:
                    try:run('docker',kind,'rm',name)
                    except Exception:pass
    print('Complete: '+str(out),flush=True)
if __name__=='__main__':main()
