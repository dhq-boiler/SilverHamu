"""Disposable resource envelope with explicitly recorded service budgets."""
import argparse, datetime, gzip, itertools, json, os, pathlib, random, secrets, subprocess, time, uuid

ROOT=pathlib.Path(__file__).resolve().parents[2]
IMAGE='catdb-resource:local'

def run(*args,timeout=600,env=None):
    p=subprocess.run(list(map(str,args)),capture_output=True,text=True,encoding='utf8',errors='replace',timeout=timeout,env=env)
    if p.returncode:raise RuntimeError(p.stderr[-2000:]+p.stdout[-1500:])
    return p.stdout.strip()

def main():
    p=argparse.ArgumentParser();p.add_argument('--read-ramp',action='store_true');p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--rows',nargs='+',type=int,default=[100000,1000000]);p.add_argument('--cpus',nargs='+',type=int,default=[1,2]);p.add_argument('--memory',nargs='+',type=int,default=[512,1024,2048]);p.add_argument('--reps',type=int,default=3);p.add_argument('--ops',type=int,default=8000);p.add_argument('--sample',type=int,choices=[0,1],default=1);p.add_argument('--rate',type=float,default=1000);p.add_argument('--recover-memory',type=int);p.add_argument('--retained-bytes',type=int);p.add_argument('--image',default=IMAGE);p.add_argument('--age-seed',action='store_true');p.add_argument('--safety-probe',action='store_true');p.add_argument('--staged-bytes',type=int);p.add_argument('--diagnostics',action='store_true');p.add_argument('--pin-reader',type=int,choices=[0,1],default=1);p.add_argument('--kernel-mode',choices=['off','trace']);p.add_argument('--storage-mode',choices=['volume','bind']);p.add_argument('--bind-root',type=pathlib.Path);p.add_argument('--preparation-budget',type=int);p.add_argument('--batch-limit',type=int,choices=[1,8]);p.add_argument('--stall-scenario',choices=['normal','single'],default='normal');a=p.parse_args()
    if a.read_ramp and (a.batch_limit or a.kernel_mode):p.error('read ramp requires ordinary service client')
    if a.preparation_budget is not None and (not a.batch_limit or not a.staged_bytes or not 0<a.preparation_budget<=a.staged_bytes):p.error('preparation budget requires batch API and must fit staged budget')
    if a.batch_limit and a.kernel_mode:p.error('batch workload owns its measurement gate')
    if a.retained_bytes is not None and a.retained_bytes<=0: p.error('--retained-bytes must be positive')
    if a.staged_bytes is not None and a.staged_bytes<=0:p.error('--staged-bytes must be positive')
    if a.safety_probe and a.staged_bytes!=4096:p.error('--safety-probe requires --staged-bytes 4096')
    if (a.storage_mode=='bind') != (a.bind_root is not None):p.error('--bind-root is required only for bind storage')
    if a.bind_root is not None:
        a.bind_root=a.bind_root.resolve();a.bind_root.mkdir(parents=True,exist_ok=False)
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
    token=secrets.token_hex(24);env=dict(os.environ,BENCH_PASSWORD=secrets.token_hex(16),BENCH_TOKEN=token)
    thread_tag='as-'+uuid.uuid4().hex[:10];uid='catdb-resource-'+uuid.uuid4().hex[:10];network=uid+'-net';seeds={};created=[];results=[]
    metadata={'commit':run('git','-C',ROOT,'rev-parse','HEAD'),'image':run('docker','image','inspect',a.image,'--format','{{.Id}}'),'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'arguments':{k:str(v) if isinstance(v,pathlib.Path) else v for k,v in vars(a).items()},'docker':json.loads(run('docker','info','--format','{{json .}}'))}
    metadata['docker']={k:metadata['docker'].get(k) for k in ['ServerVersion','NCPU','MemTotal','Driver','OSType','Architecture','KernelVersion']}
    (out/'environment.json').write_text(json.dumps(metadata,indent=2))
    mount=['-v',str(ROOT/'benchmarks')+':/bench-src:ro','-e','PYTHONPATH=/bench:/bench-src','-e','BENCH_TOKEN','-e','BENCH_PASSWORD']
    if a.batch_limit:mount+=['-e',f'CATDB_BATCH_LIMIT={a.batch_limit}','-e',f'CATDB_STALL_SCENARIO={a.stall_scenario}','-e','CATDB_BENCH_STALL=1']
    if a.preparation_budget:mount+=['-e',f'CATDB_PREPARATION_BUDGET={a.preparation_budget}']
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
            if a.age_seed:
                result=run('docker','run','--rm','--network',network,'--cpus','2','--memory','4g','--memory-swap','4g',*mount,'-e',f'BENCH_ROWS={rows}','-v',seed+':/data',a.image,'python','/bench-src/aged-service/age_seed.py',env=env)
                (out/f'aged-seed-{rows}.json').write_text(json.dumps(json.loads(result),indent=2))
        cases=list(itertools.product(a.rows,a.cpus,a.memory))
        for rep in range(1,a.reps+1):
            order=cases.copy();random.Random(833+rep).shuffle(order)
            for index,(rows,cpus,memory) in enumerate(order):
                name=f'r{rep}-n{rows}-c{cpus}-m{memory}';caseout=out/name;caseout.mkdir();vol=uid+f'-v-{rep}-{index}';client=vol+'-client';server=vol+'-db'
                entry={'name':name,'rows':rows,'cpus':cpus,'memory_mib':memory,'rep':rep,'sample':a.sample,'retained_bytes':a.retained_bytes,'status':'running','verification':[],'measurements':[]};results.append(entry);save()
                print(name,'start',flush=True)
                local=[]
                try:
                    data_source=vol;control='/data';case_mount=mount
                    if a.storage_mode=='bind':
                        data_source=str(a.bind_root/name);pathlib.Path(data_source).mkdir(exist_ok=False)
                    else:
                        run('docker','volume','create',vol);local.append(('volume',vol))
                    if a.storage_mode:
                        control='/control';cv=vol+'-control';run('docker','volume','create',cv);local.append(('volume',cv))
                        case_mount=mount+['-v',cv+':/control','-e','CATDB_BENCH_CONTROL=/control']
                    run('docker','run','--rm','--network',network,'-v',seeds[rows]+':/seed:ro','-v',data_source+':/data',*case_mount,a.image,'python','-c',"import os,pathlib,shutil; control=pathlib.Path(os.environ.get('CATDB_BENCH_CONTROL','/data'));[shutil.copy2(p,(pathlib.Path('/data') if p.suffix in ['.wal','.ndb'] else control)/p.name) for p in pathlib.Path('/seed').iterdir()]")
                    run('docker','run','-d','--name',client,'--network',network,'--cpus','2','--memory','2g','--memory-swap','2g',*case_mount,'-e',f'BENCH_ROWS={rows}','-v',data_source+':/data',*(['-v',str(caseout)+':/host-results'] if a.kernel_mode else []),a.image,env=env);local.append(('container',client))
                    run('docker','exec',client,'python','-c',"import os,pathlib;p=pathlib.Path(os.environ.get('CATDB_BENCH_CONTROL','/data'))/'token';p.write_text(os.environ['BENCH_TOKEN']);p.chmod(0o400)")
                    def execute(action,*extra,timeout=600):
                        return json.loads(run('docker','exec',client,'python',('/bench-src/wire-buffer/ramp_client.py' if a.read_ramp else '/bench-src/constrained-backlog/client.py' if a.batch_limit else '/bench-src/aged-sync/client.py' if a.kernel_mode else '/bench-src/resource-envelope/client.py'),action,*extra,timeout=timeout))
                    def boot(phase):
                        start=time.monotonic()
                        run('docker','run','-d','--name',server,'--network',network,'--network-alias','database','--cpus',cpus,'--memory',f'{memory}m','--memory-swap',f'{memory}m','--pids-limit','128',*case_mount,*(['-e','CATDB_BENCH_THREAD_NAME='+thread_tag] if a.kernel_mode else []),*(['-e','CATDB_DIAGNOSTICS=1'] if a.diagnostics else []),'-e',f'SAMPLE={a.sample}','-e',f'PHASE={phase}','-e',f'RETAINED_BYTES={a.retained_bytes or ""}','-e',f'STAGED_BYTES={a.staged_bytes or ""}','-v',data_source+':/data',a.image,'python','/bench-src/resource-envelope/supervisor.py',env=env)
                        local.append(('container',server))
                        ready=execute('ready',timeout=75);ready['launch_to_ready_ms']=(time.monotonic()-start)*1000
                        limits=json.loads(run('docker','exec',client,'cat',f'{control}/{phase}-limits.json'))
                        assert int(limits['memory.max'])==memory*1024*1024 and limits['memory.swap.max']=='0'
                        quota,period=map(int,limits['cpu.max'].split());assert quota/period==cpus
                        entry[phase]={'timing':ready,'limits':limits}
                        try:entry[phase]['process_ready']=json.loads(run('docker','exec',client,'cat',f'{control}/{phase}-ready.json'))
                        except RuntimeError:pass
                    boot('boot')
                    if a.storage_mode:
                        entry['storage_probe']=json.loads(run('docker','exec',client,'python','/bench-src/storage-path/probe.py'))
                        entry['mounts']=json.loads(run('docker','inspect',server,'--format','{{json .Mounts}}'));save()
                    entry['verification'].append({'phase':'initial',**execute('verify')})
                    workloads=['ramp'] if a.read_ramp else ['read','mixed'] if rep%2 else ['mixed','read']
                    for work in workloads:
                        begin=time.monotonic_ns()
                        if a.kernel_mode:
                            import importlib.util
                            spec=importlib.util.spec_from_file_location('aged_sync_host',ROOT/'benchmarks/aged-sync/host.py');hook=importlib.util.module_from_spec(spec);spec.loader.exec_module(hook)
                            m=hook.measure(execute,work,a.ops,a.rate,a.pin_reader,server,a.image,caseout/(work+'-kernel'),a.kernel_mode)
                        else:m=execute('measure','--work',work,'--ops',str(a.ops),'--rate',str(a.rate),'--pin-reader',str(a.pin_reader))
                        m['host_begin_ns']=begin;m['host_end_ns']=time.monotonic_ns();entry['measurements'].append(m)
                        assert not m['errors'],m['errors'][:2]
                        entry['verification'].append({'phase':work,**execute('verify')});save()
                    if a.safety_probe:
                        entry['safety_probe']=execute('safety');entry['verification'].append({'phase':'safety',**execute('verify')});save()
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
                    for pattern in ['boot-command.json','reopen-command.json','rescue-command.json','boot-resources.jsonl','reopen-resources.jsonl','rescue-resources.jsonl','boot-exit.json','reopen-exit.json','rescue-exit.json','boot-server.log','reopen-server.log','rescue-server.log','read-raw.json','mixed-raw.json','ramp-raw.json']:
                        try:run('docker','cp',client+':'+control+'/'+pattern,caseout/pattern)
                        except Exception:pass
                    for kind,n in reversed(local):remove(kind,n)
                    save()
                print(name,entry['status'],flush=True)
    finally:
        for kind,n in reversed(created):remove(kind,n)
        remove('network',network)

if __name__=='__main__': main()
