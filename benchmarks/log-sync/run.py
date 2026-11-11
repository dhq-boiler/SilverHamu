"""Run only disposable, uniquely named Docker fixtures. Requires Docker Desktop/Linux."""
import argparse, datetime, gzip, hashlib, itertools, json, os, pathlib, random, secrets, subprocess, sys, time, uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
IMAGES={'catdb-current':'catdb-update-handoff:local','catdb-candidate':'catdb-sync-cohort:local',
        'postgres':'postgres@sha256:3a4264520e121176619f54ff867d12774bc81e95e7b84db58d63bccdb6144b81',
        'oracle':'container-registry.oracle.com/database/free@sha256:cf540c3fa190d7cffad08c491652ac07fc70314e510f6b87890449517d565e94'}

def run(*args,env=None,timeout=600):
    r=subprocess.run(list(args),capture_output=True,text=True,encoding='utf-8',errors='replace',env=env,timeout=timeout)
    if r.returncode:raise RuntimeError(r.stderr[-4000:]+r.stdout[-1000:])
    return r.stdout.strip()

def main():
    p=argparse.ArgumentParser();p.add_argument('--out',required=True,type=pathlib.Path);p.add_argument('--rows',type=int,default=100000);p.add_argument('--ops',type=int,default=8000);p.add_argument('--reps',type=int,default=3);p.add_argument('--engines',nargs='+',default=list(IMAGES));p.add_argument('--clients',type=int,nargs='+',default=[1,4]);p.add_argument('--workloads',nargs='+',default=['update']);args=p.parse_args()
    out=args.out.resolve();out.mkdir(parents=True,exist_ok=False)
    environment={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'rows':args.rows,'value_bytes':256,'ops_per_case':args.ops,'clients':args.clients,'workloads':args.workloads,'reps':args.reps,'git_commit':run('git','-C',str(ROOT),'rev-parse','HEAD'),'git_dirty':bool(run('git','-C',str(ROOT),'status','--porcelain')),'docker':json.loads(run('docker','info','--format','{{json .}}')),'images':{},'source_sha256':{},'runs':[], 'skipped':{'sql-server':'SQL Server 2022 Developer/Express/Evaluation license section 5 requires prior written permission for disclosure; permission not obtained, not measured.'}}
    # Publish only hardware/runtime fields; Docker info can contain workstation names and paths.
    environment['docker']={k:environment['docker'].get(k) for k in ['ServerVersion','NCPU','MemTotal','Driver','OSType','Architecture','KernelVersion']}
    for f in ['runner.py','log-sync/run.py','log-sync/client.py']:
        environment['source_sha256'][f]=hashlib.sha256((ROOT/'benchmarks'/f).read_bytes()).hexdigest()
    for engine in args.engines:
        image=IMAGES.get(engine,'catdb-comparison:local')
        environment['images'][engine]=json.loads(run('docker','image','inspect',image,'--format','{{json .RepoDigests}}')) or json.loads(run('docker','image','inspect',image,'--format','{{json .Id}}'))
    environment['engine_source_sha256']={str(f.relative_to(ROOT)):hashlib.sha256(f.read_bytes()).hexdigest() for f in [ROOT/'Cargo.toml',ROOT/'src/group_commit.rs',ROOT/'src/transactions.rs',ROOT/'src/service.rs']}
    environment['client_image_id']=run('docker','image','inspect','catdb-comparison:local','--format','{{.Id}}')
    all_results=[]
    def save():
        (out/'environment.json').write_text(json.dumps(environment,indent=2)+'\n',encoding='utf-8')
        (out/'results.json').write_text(json.dumps(all_results,indent=2)+'\n',encoding='utf-8')
    save()
    for rep in range(1,args.reps+1):
        order=args.engines[:] if rep%2 else list(reversed(args.engines))
        if rep==3:order=order[2:]+order[:2]
        for engine in order:
            is_catdb=engine in ['catdb-current','catdb-candidate']
            actual_engine='catdb-service' if is_catdb else engine
            prefix='catdb-bench-'+uuid.uuid4().hex[:10];network=prefix+'-net';volume=prefix+'-data';db=prefix+'-db';client=prefix+'-client';containers=[]
            env=dict(os.environ);env.update(BENCH_PASSWORD='B!'+secrets.token_hex(18),BENCH_TOKEN=secrets.token_hex(32),BENCH_ROWS=str(args.rows),BENCH_OPS=str(args.ops))
            entry={'engine':engine,'rep':rep,'status':'running','verification':[]};environment['runs'].append(entry);save()
            print(f'{engine} rep {rep}: preparing',flush=True)
            try:
                run('docker','network','create','--internal',network);run('docker','volume','create',volume)
                common=['--cpus','2','--memory','4g','--network',network]
                run('docker','run','-d','--name',client,*common,'-v',volume+':/data','-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','BENCH_PASSWORD','-e','BENCH_TOKEN','-e','BENCH_ROWS','-e','BENCH_OPS','catdb-comparison:local',env=env);containers.append(client)
                def execute(action,*extra):
                    return json.loads(run('docker','exec','-e','PYTHONPATH=/bench',client,'python','/bench-src/log-sync/client.py',action,actual_engine,*map(str,extra),timeout=600))
                if is_catdb:
                    execute('init')
                    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path('/data/token');p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400);os.chown(p,10001,10001)")
                    run('docker','run','-d','--name',db,*common,'--network-alias','database','--user','10001:10001','-v',volume+':/data',IMAGES[engine],'silverhamu-server','--db','/data/catdb.wal','--bind','0.0.0.0:7400','--allow-remote','--token-file','/data/token','--transaction-timeout-secs','3600',env=env);containers.append(db)
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
                            if is_catdb:
                                run('docker','exec','-e','PYTHONPATH=/bench',client,'python','-c',"import os;from catdb_client import Client;c=Client('database',7400,os.environ['BENCH_TOKEN']);assert c.request(op='ping')['ok'];c.close()")
                            else:execute('metadata')
                            break
                        except RuntimeError:
                            if attempt==89:raise
                            time.sleep(2)
                if not is_catdb:execute('init')
                entry['python_packages']=run('docker','exec',client,'pip','freeze').splitlines();entry['metadata']=execute('metadata');entry['verification'].append({'stage':'initial',**execute('verify')});save()
                cases=list(itertools.product(args.workloads,args.clients));random.Random(912+rep).shuffle(cases)
                for work,clients in cases:
                    result=execute('measure','--workload',work,'--clients',clients,'--rep',rep)
                    result['engine']=engine
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
