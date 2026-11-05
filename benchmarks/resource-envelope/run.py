"""Disposable resource envelope with explicitly recorded service budgets."""
import argparse, datetime, gzip, itertools, json, os, pathlib, random, secrets, subprocess, time, uuid

ROOT=pathlib.Path(__file__).resolve().parents[2]
IMAGE='catdb-resource:local'

def run(*args,timeout=600,env=None):
    p=subprocess.run(list(map(str,args)),capture_output=True,text=True,encoding='utf8',errors='replace',timeout=timeout,env=env)
    if p.returncode:raise RuntimeError(p.stderr[-2000:]+p.stdout[-1500:])
    return p.stdout.strip()

def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--rows',nargs='+',type=int,default=[100000,1000000]);p.add_argument('--cpus',nargs='+',type=int,default=[1,2]);p.add_argument('--memory',nargs='+',type=int,default=[512,1024,2048]);p.add_argument('--reps',type=int,default=3);p.add_argument('--ops',type=int,default=8000);p.add_argument('--sample',type=int,choices=[0,1],default=1);p.add_argument('--rate',type=float,default=1000);p.add_argument('--recover-memory',type=int);p.add_argument('--retained-bytes',type=int);p.add_argument('--image',default=IMAGE);a=p.parse_args()
    if a.retained_bytes is not None and a.retained_bytes<=0: p.error('--retained-bytes must be positive')
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
    token=secrets.token_hex(24);env=dict(os.environ,BENCH_PASSWORD=secrets.token_hex(16),BENCH_TOKEN=token)
    uid='catdb-resource-'+uuid.uuid4().hex[:10];network=uid+'-net';seeds={};created=[];results=[]
    metadata={'commit':run('git','-C',ROOT,'rev-parse','HEAD'),'image':run('docker','image','inspect',a.image,'--format','{{.Id}}'),'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'arguments':{k:str(v) if isinstance(v,pathlib.Path) else v for k,v in vars(a).items()},'docker':json.loads(run('docker','info','--format','{{json .}}'))}
    metadata['docker']={k:metadata['docker'].get(k) for k in ['ServerVersion','NCPU','MemTotal','Driver','OSType','Architecture','KernelVersion']}
    (out/'environment.json').write_text(json.dumps(metadata,indent=2))
    mount=['-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','PYTHONPATH=/bench:/bench-src','-e','BENCH_TOKEN','-e','BENCH_PASSWORD']
    def save(): (out/'results.json').write_text(json.dumps(results,indent=2))
    def remove(kind,name):
        try:run('docker',kind,'rm',*(['-f'] if kind=='container' else []),name,timeout=60)
        except Exception as e: print('cleanup:',name,str(e),flush=True)
    try:
        run('docker','network','create','--internal',network)
        for rows in a.rows:
            seed=uid+'-seed-'+str(rows);run('docker','volume','create',seed);created.append(('volume',seed));seeds[rows]=seed
            print('prepare seed',rows,flush=True)
            run('docker','run','--rm','--network',network,'--cpus','2','--memory','4g','--memory-swap','4g',*mount,'-e',f'BENCH_ROWS={rows}','-v',seed+':/data',a.image,'python','/bench-src/runner.py','init','catdb-embedded',env=env)
        cases=list(itertools.product(a.rows,a.cpus,a.memory))
        for rep in range(1,a.reps+1):
            order=cases.copy();random.Random(833+rep).shuffle(order)
            for index,(rows,cpus,memory) in enumerate(order):
                name=f'r{rep}-n{rows}-c{cpus}-m{memory}';caseout=out/name;caseout.mkdir();vol=uid+f'-v-{rep}-{index}';client=vol+'-client';server=vol+'-db'
                entry={'name':name,'rows':rows,'cpus':cpus,'memory_mib':memory,'rep':rep,'sample':a.sample,'retained_bytes':a.retained_bytes,'status':'running','verification':[],'measurements':[]};results.append(entry);save()
                print(name,'start',flush=True)
                local=[]
                try:
                    run('docker','volume','create',vol);local.append(('volume',vol))
                    run('docker','run','--rm','--network',network,'-v',seeds[rows]+':/seed:ro','-v',vol+':/data',a.image,'python','-c',"import pathlib,shutil;[shutil.copy2(p,pathlib.Path('/data')/p.name) for p in pathlib.Path('/seed').iterdir()]")
                    run('docker','run','-d','--name',client,'--network',network,'--cpus','2','--memory','2g','--memory-swap','2g',*mount,'-e',f'BENCH_ROWS={rows}','-v',vol+':/data',a.image,env=env);local.append(('container',client))
                    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path('/data/token');p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400)")
                    def execute(action,*extra,timeout=600):
                        return json.loads(run('docker','exec',client,'python','/bench-src/resource-envelope/client.py',action,*extra,timeout=timeout))
                    def boot(phase):
                        start=time.monotonic()
                        run('docker','run','-d','--name',server,'--network',network,'--network-alias','database','--cpus',cpus,'--memory',f'{memory}m','--memory-swap',f'{memory}m','--pids-limit','128',*mount,'-e',f'SAMPLE={a.sample}','-e',f'PHASE={phase}','-e',f'RETAINED_BYTES={a.retained_bytes or ""}','-v',vol+':/data',a.image,'python','/bench-src/resource-envelope/supervisor.py',env=env)
                        local.append(('container',server))
                        ready=execute('ready',timeout=75);ready['launch_to_ready_ms']=(time.monotonic()-start)*1000
                        limits=json.loads(run('docker','exec',client,'cat',f'/data/{phase}-limits.json'))
                        assert int(limits['memory.max'])==memory*1024*1024 and limits['memory.swap.max']=='0'
                        quota,period=map(int,limits['cpu.max'].split());assert quota/period==cpus
                        entry[phase]={'timing':ready,'limits':limits}
                        try:entry[phase]['process_ready']=json.loads(run('docker','exec',client,'cat',f'/data/{phase}-ready.json'))
                        except RuntimeError:pass
                    boot('boot')
                    entry['verification'].append({'phase':'initial',**execute('verify')})
                    workloads=['read','mixed'] if rep%2 else ['mixed','read']
                    for work in workloads:
                        begin=time.monotonic_ns()
                        m=execute('measure','--work',work,'--ops',str(a.ops),'--rate',str(a.rate));m['host_begin_ns']=begin;m['host_end_ns']=time.monotonic_ns();entry['measurements'].append(m)
                        assert not m['errors'],m['errors'][:2]
                        entry['verification'].append({'phase':work,**execute('verify')});save()
                    # Killing only the owned server child also tests acknowledged writes without a graceful DB close.
                    run('docker','exec',server,'python','-c',"import os,signal,pathlib;pid=int(pathlib.Path('/proc/1/task/1/children').read_text().split()[0]);os.kill(pid,signal.SIGKILL)")
                    run('docker','wait',server,timeout=60);entry['stop_state']=json.loads(run('docker','inspect',server,'--format','{{json .State}}'));remove('container',server);local.remove(('container',server))
                    boot('reopen');entry['verification'].append({'phase':'reopen',**execute('verify')})
                    run('docker','stop','--time','30',server,timeout=45)
                    entry['status']='passed'
                except Exception as e:
                    entry['status']='failed';entry['error']=str(e)
                    try:entry['failure_state']=json.loads(run('docker','inspect',server,'--format','{{json .State}}'))
                    except Exception:pass
                    print(name,'failed',str(e)[:180],flush=True)
                    if a.recover_memory and ('container',server) in local:
                        remove('container',server);local.remove(('container',server))
                        memory=a.recover_memory
                        try:
                            boot('rescue');entry['verification'].append({'phase':'rescue',**execute('verify')})
                            entry['rescue_verified']=True
                        except Exception as rescue:entry['rescue_error']=str(rescue)
                finally:
                    if ('container',server) in local:
                        try:run('docker','stop','--time','10',server,timeout=20)
                        except Exception:pass
                    for pattern in ['boot-command.json','reopen-command.json','rescue-command.json','boot-resources.jsonl','reopen-resources.jsonl','rescue-resources.jsonl','boot-exit.json','reopen-exit.json','rescue-exit.json','boot-server.log','reopen-server.log','rescue-server.log','read-raw.json','mixed-raw.json']:
                        try:run('docker','cp',client+':/data/'+pattern,caseout/pattern)
                        except Exception:pass
                    for kind,n in reversed(local):remove(kind,n)
                    save()
                print(name,entry['status'],flush=True)
    finally:
        for kind,n in reversed(created):remove(kind,n)
        remove('network',network)

if __name__=='__main__': main()
